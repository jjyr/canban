//! Network listener configuration, Bearer Token auth, Tailscale/SSH discovery,
//! TCP host probing, SSH pairing, and Linux/macOS user service management.
use super::*;
use std::{
    net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs},
    process::Stdio,
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

/// DEPRECATED: legacy static `server.token` path; retained only to auto-upgrade older hosts.
pub fn token_path(home: &Path) -> PathBuf {
    canban_transport::auth::legacy_token_path(home)
}

/// DEPRECATED: read legacy static `server.token` if present.
pub fn read_server_token(home: &Path) -> Option<String> {
    canban_transport::auth::read_legacy_server_token(home)
}

/// DEPRECATED: generate legacy static `server.token` for backward-compatibility tests.
pub fn ensure_server_token(home: &Path) -> Result<String> {
    canban_transport::auth::ensure_legacy_server_token(home)
}

/// DEPRECATED: Constant-time token comparison for legacy `server.token` verification.
pub fn verify_token(expected: &str, provided: &str) -> bool {
    canban_transport::auth::verify_legacy_token(expected, provided)
}

pub fn is_tailscale_ip(ip: IpAddr) -> bool {
    canban_types::is_tailscale_ip(ip)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailscaleSelfIdentity {
    pub user_id: u64,
    pub stable_id: String,
    pub ips: Vec<String>,
}

/// Extracts the daemon host's personal Tailscale `UserID` and identity from `tailscale status --json`.
/// Tagged servers (`Self.Tags` non-empty) have no personal `UserID` match and are rejected.
pub fn parse_tailscale_self_identity(raw: &str) -> Result<TailscaleSelfIdentity> {
    let v: Value = serde_json::from_str(raw).context("Invalid tailscale status JSON")?;
    let backend_state = v["BackendState"].as_str().unwrap_or_default();
    ensure!(
        backend_state.is_empty() || backend_state == "Running",
        "Tailscale is not running (state: {backend_state})"
    );
    let self_obj = &v["Self"];
    let self_tags_empty = self_obj["Tags"].as_array().is_none_or(|a| a.is_empty());
    ensure!(
        self_tags_empty,
        "Tagged Tailscale nodes do not carry a personal UserID"
    );
    let user_id = self_obj["UserID"].as_u64().unwrap_or(0);
    ensure!(user_id > 0, "Tailscale self UserID unavailable");
    let stable_id = self_obj["ID"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let ips = self_obj["TailscaleIPs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(TailscaleSelfIdentity {
        user_id,
        stable_id,
        ips,
    })
}

/// Validates `tailscale whois --json` output against `self_user_id` for same-account auto-auth.
pub fn parse_tailscale_whois_json(
    raw: &str,
    self_user_id: u64,
) -> Option<canban_transport::TailscalePeerIdentity> {
    if self_user_id == 0 {
        return None;
    }
    let v: Value = serde_json::from_str(raw).ok()?;
    let node = &v["Node"];
    if node["Tags"].as_array().is_some_and(|a| !a.is_empty()) {
        return None;
    }
    if node["MachineAuthorized"].as_bool() == Some(false) {
        return None;
    }
    if node["IsShareeNode"].as_bool() == Some(true) {
        return None;
    }
    let node_user = node["User"].as_u64().unwrap_or(0);
    let profile_id = v["UserProfile"]["ID"].as_u64().unwrap_or(0);
    if node_user != self_user_id || profile_id != self_user_id {
        return None;
    }
    let node_id = node["StableID"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| node["ID"].as_u64().map(|n| n.to_string()))?;
    let dns_short = node["Name"]
        .as_str()
        .map(|s| s.trim_end_matches('.'))
        .and_then(|s| s.split('.').next())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let device_name = node["ComputedName"]
        .as_str()
        .or_else(|| node["Hostinfo"]["Hostname"].as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "localhost")
        .or(dns_short)
        .unwrap_or("Tailscale device")
        .to_owned();
    let login_name = v["UserProfile"]["LoginName"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned();
    Some(canban_transport::TailscalePeerIdentity {
        node_id,
        device_name,
        login_name,
    })
}

/// Verifies incoming Tailscale TCP connections against the local `tailscaled` daemon (`whois`).
pub struct SystemTailscaleVerifier {
    cached_self_user: Mutex<Option<(u64, std::time::Instant)>>,
}

impl Default for SystemTailscaleVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemTailscaleVerifier {
    pub fn new() -> Self {
        Self {
            cached_self_user: Mutex::new(None),
        }
    }

    fn self_user_id(&self) -> Option<u64> {
        const SELF_CACHE_TTL: Duration = Duration::from_secs(60);
        if let Some((uid, fetched_at)) = *self.cached_self_user.lock().unwrap()
            && fetched_at.elapsed() < SELF_CACHE_TTL
        {
            return Some(uid);
        }
        let status_json = run_tailscale(&["status", "--json"]).ok()?;
        let self_id = parse_tailscale_self_identity(&status_json).ok()?;
        *self.cached_self_user.lock().unwrap() = Some((self_id.user_id, std::time::Instant::now()));
        Some(self_id.user_id)
    }
}

impl canban_transport::TailscalePeerVerifier for SystemTailscaleVerifier {
    fn verify_peer(
        &self,
        peer_addr: SocketAddr,
    ) -> Option<canban_transport::TailscalePeerIdentity> {
        if !is_tailscale_ip(peer_addr.ip()) {
            return None;
        }
        let self_user_id = self.self_user_id()?;
        let whois_json = run_tailscale(&["whois", "--json", &peer_addr.to_string()]).ok()?;
        parse_tailscale_whois_json(&whois_json, self_user_id)
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
        match canban_exec::command(&bin)
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
        && let Ok(out) = canban_exec::command("ip")
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
            let is_safe_default = sa.ip().is_loopback() || is_tailscale_ip(sa.ip());
            if !is_safe_default {
                ensure!(
                    cfg.allow_insecure_bind,
                    "Refusing to bind Canban TCP listener to {sa}: by default only loopback or Tailscale addresses are permitted. Pass `--allow-insecure-bind` if this interface is protected by your own VPN (e.g. WireGuard/ZeroTier) or trusted private network."
                );
                eprintln!(
                    "Warning: Canban TCP listener binding to {sa} (--allow-insecure-bind); ensure this network path is encrypted (e.g. WireGuard/ZeroTier/VPN) or trusted."
                );
            }
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

fn is_mobile_or_tv_os(os: &str) -> bool {
    let lower = os.trim().to_ascii_lowercase();
    matches!(lower.as_str(), "ios" | "android" | "tvos")
}

/// Collects candidate remote Canban TCP endpoints from saved `hosts.json` + online same-user Tailscale peers.
pub fn collect_canban_discovery_candidates(
    saved_hosts: &[SavedHost],
    status_json: Option<&str>,
    self_server_id: &str,
) -> (Vec<DiscoveredCanbanHost>, bool) {
    let mut candidates = Vec::new();
    let mut seen_addrs = std::collections::HashSet::new();
    let mut seen_hosts = std::collections::HashSet::new();

    for host in saved_hosts {
        if !host.enabled {
            continue;
        }
        if host.server_id.as_deref() == Some(self_server_id) {
            continue;
        }
        let HostTransport::Tcp { addr, .. } = &host.transport else {
            continue;
        };
        let norm_addr = canban_types::normalize_daemon_addr(addr);
        if norm_addr.is_empty() || !seen_addrs.insert(norm_addr.clone()) {
            continue;
        }
        let host_part = canban_types::extract_daemon_host(&norm_addr).to_ascii_lowercase();
        if !host_part.is_empty() {
            seen_hosts.insert(host_part);
        }
        candidates.push(DiscoveredCanbanHost {
            name: host.name.clone(),
            addr: norm_addr,
            server_id: host.server_id.clone(),
            os: String::new(),
            rtt_ms: 0,
            linked_on_host: true,
            is_tailscale: canban_types::is_tailscale_addr(addr),
        });
    }

    let Some(raw) = status_json else {
        return (candidates, false);
    };
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return (candidates, false);
    };
    let backend_state = v["BackendState"].as_str().unwrap_or_default();
    let ts_available = backend_state == "Running";
    let self_user_id = v["Self"]["UserID"].as_u64().unwrap_or(0);
    let self_tags_empty = v["Self"]["Tags"].as_array().is_none_or(|a| a.is_empty());
    let mut self_ips = std::collections::HashSet::new();
    if let Some(arr) = v["Self"]["TailscaleIPs"].as_array() {
        for ip in arr.iter().filter_map(Value::as_str) {
            self_ips.insert(ip.to_ascii_lowercase());
        }
    }

    if !ts_available || self_user_id == 0 || !self_tags_empty {
        return (candidates, ts_available);
    }

    if let Some(peer_map) = v["Peer"].as_object() {
        for peer in peer_map.values() {
            if peer["Online"].as_bool() != Some(true)
                || peer["IsShareeNode"].as_bool() == Some(true)
            {
                continue;
            }
            if peer["Tags"].as_array().is_some_and(|a| !a.is_empty()) {
                continue;
            }
            if peer["UserID"].as_u64() != Some(self_user_id) {
                continue;
            }
            let os = peer["OS"].as_str().unwrap_or("unknown").to_owned();
            if is_mobile_or_tv_os(&os) {
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
            let Some(preferred_ip) = ips
                .iter()
                .find(|ip| ip.contains('.'))
                .or_else(|| ips.first())
                .map(String::as_str)
            else {
                continue;
            };
            let raw_dns = peer["DNSName"]
                .as_str()
                .unwrap_or_default()
                .trim_end_matches('.');
            let dns_lower = raw_dns.to_ascii_lowercase();
            let is_self = ips
                .iter()
                .any(|ip| self_ips.contains(&ip.to_ascii_lowercase()));
            let already_seen = ips
                .iter()
                .any(|ip| seen_hosts.contains(&ip.to_ascii_lowercase()))
                || (!dns_lower.is_empty() && seen_hosts.contains(&dns_lower));
            if is_self || already_seen {
                continue;
            }
            let norm_addr = canban_types::normalize_daemon_addr(preferred_ip);
            if norm_addr.is_empty() || !seen_addrs.insert(norm_addr.clone()) {
                continue;
            }
            for ip in &ips {
                seen_hosts.insert(ip.to_ascii_lowercase());
            }
            if !dns_lower.is_empty() {
                seen_hosts.insert(dns_lower);
            }
            let raw_host = peer["HostName"].as_str().unwrap_or_default().trim();
            let name = if !raw_host.is_empty() && raw_host != "localhost" {
                raw_host.to_owned()
            } else if let Some((first, _)) = raw_dns.split_once('.') {
                first.to_owned()
            } else {
                preferred_ip.to_owned()
            };
            candidates.push(DiscoveredCanbanHost {
                name,
                addr: norm_addr,
                server_id: None,
                os,
                rtt_ms: 0,
                linked_on_host: false,
                is_tailscale: true,
            });
        }
    }

    (candidates, ts_available)
}

/// Performs a one-shot bounded parallel TCP probe across `candidates`, keeping only reachable Canban hosts.
pub fn probe_canban_discovery_candidates(
    mut candidates: Vec<DiscoveredCanbanHost>,
    timeout: Duration,
) -> Vec<DiscoveredCanbanHost> {
    if candidates.is_empty() {
        return candidates;
    }
    let total = candidates.len();
    let workers = total.min(16);
    let next_idx = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<Mutex<Option<u64>>> = (0..total).map(|_| Mutex::new(None)).collect();

    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next_idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= total {
                        break;
                    }
                    if let Ok(rtt) = probe_tcp_reachable(&candidates[i].addr, timeout) {
                        *results[i].lock().unwrap() = Some(rtt);
                    }
                }
            });
        }
    });

    let mut reachable = Vec::new();
    for (cand, rtt_slot) in candidates.drain(..).zip(results) {
        if let Some(rtt_ms) = rtt_slot.into_inner().unwrap() {
            let mut item = cand;
            item.rtt_ms = rtt_ms;
            reachable.push(item);
        }
    }
    reachable.sort_by(|a, b| {
        b.linked_on_host
            .cmp(&a.linked_on_host)
            .then_with(|| a.rtt_ms.cmp(&b.rtt_ms))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    reachable
}

/// Discovers reachable Canban hosts across this machine's saved TCP hosts and an optional Tailscale status snapshot.
pub fn discover_canban_hosts_with_status(
    home: &Path,
    self_server_id: &str,
    status_json: Option<&str>,
) -> (Vec<DiscoveredCanbanHost>, bool) {
    let hosts_file: HostsFile = std::fs::read(home.join("config/hosts.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let (candidates, ts_available) =
        collect_canban_discovery_candidates(&hosts_file.hosts, status_json, self_server_id);
    let reachable = probe_canban_discovery_candidates(candidates, Duration::from_millis(1500));
    (reachable, ts_available)
}

/// Discovers reachable Canban hosts across this machine's saved TCP hosts and same-user Tailscale peers.
pub fn discover_canban_hosts(
    home: &Path,
    self_server_id: &str,
) -> (Vec<DiscoveredCanbanHost>, bool) {
    let status_json = run_tailscale(&["status", "--json"]).ok();
    discover_canban_hosts_with_status(home, self_server_id, status_json.as_deref())
}

/// Probe whether a remote Canban TCP endpoint accepts connections and measure connect RTT (ms).
pub fn probe_tcp_reachable(raw_addr: &str, timeout: Duration) -> Result<u64> {
    let normalized = canban_types::normalize_daemon_addr(raw_addr);
    ensure!(!normalized.is_empty(), "Host address is empty");
    let sock_addr = normalized
        .to_socket_addrs()
        .with_context(|| format!("Could not resolve {normalized}"))?
        .next()
        .with_context(|| format!("No socket address resolved for {normalized}"))?;
    let start = std::time::Instant::now();
    let stream = TcpStream::connect_timeout(&sock_addr, timeout)
        .with_context(|| format!("Could not connect to {normalized}"))?;
    drop(stream);
    Ok(start.elapsed().as_millis() as u64)
}

/// Attempt non-interactive SSH pairing (`BatchMode=yes`) against `ssh_target`.
/// If `canban` is already installed on the remote machine, enables `--tailscale` (when requested),
/// ensures the user service is running, issues a revocable device token for this host, and returns
/// `{ addr, token, server_id, hostname, platform, home }`.
pub fn pair_via_ssh_noninteractive(
    ssh_target: &str,
    prefer_tailscale: bool,
    port: u16,
) -> Result<Value> {
    let mode_flag = if prefer_tailscale { "--tailscale" } else { "" };
    let device_name_quoted = shell_single_quote(&hostname_string());
    let remote_script = format!(
        r#"export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH";
if ! command -v canban >/dev/null 2>&1; then
  echo '{{"error":"canban_not_installed"}}'
  exit 0
fi
canban service install {mode_flag} --port {port} >/dev/null 2>&1 || true
canban pair {mode_flag} --port {port} --device-name {device_name_quoted} --json 2>/dev/null || canban pair {mode_flag} --port {port} --json"#
    );
    let out = canban_exec::command("ssh")
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
  ASSET="canban-cli-linux-${{ARCH}}.tar.gz"
  RELEASE_URL="https://github.com/{repo}/releases/latest/download/${{ASSET}}"
  TMP=$(mktemp -d)
  echo "    Checking latest release asset ${{ASSET}}..."
  if curl -fsLI --connect-timeout 10 "$RELEASE_URL" >/dev/null 2>&1; then
    echo "    Downloading ${{ASSET}}..."
    curl -fL --progress-bar --retry 3 --retry-delay 2 --connect-timeout 15 "$RELEASE_URL" -o "$TMP/canban.tar.gz"
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
    canban_exec::command("hostname")
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
        let active = canban_exec::command("systemctl")
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
        let active = canban_exec::command("launchctl")
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
    allow_insecure_bind: bool,
) -> Result<Value> {
    let cfg = ServerListenConfig {
        tailscale,
        port,
        listen,
        allow_insecure_bind,
        trust_tailscale_self: true,
    };
    save_listen_config(home, &cfg)?;
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
        let _ = canban_exec::command("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        let enabled = canban_exec::command("systemctl")
            .args(["--user", "enable", "--now", "canban.service"])
            .status()
            .is_ok_and(|s| s.success());
        if let Ok(user) = std::env::var("USER") {
            let _ = canban_exec::command("loginctl")
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
        let _ = canban_exec::command("launchctl")
            .args(["unload", &plist_path.to_string_lossy()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let loaded = canban_exec::command("launchctl")
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
        assert!(
            cmd.contains("--progress-bar")
                && !cmd.contains("-o \"$TMP/canban.tar.gz\" 2>/dev/null"),
            "bootstrap command must show visible curl download progress instead of silencing stderr:\n{cmd}"
        );
        for shell in ["/bin/sh", "/bin/zsh"] {
            if Path::new(shell).is_file() {
                let status = canban_exec::command(shell)
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
        let inner_status = canban_exec::command("/bin/sh")
            .args(["-c", &verify_inner])
            .status()
            .expect("verify inner remote script syntax");
        assert!(
            inner_status.success(),
            "remote inner sh script failed syntax check:\n{cmd}"
        );
    }

    #[test]
    fn resolve_bind_addrs_guards_non_tailscale_addresses_unless_allow_insecure_bind() {
        let safe_loopback = ServerListenConfig {
            tailscale: false,
            port: 7680,
            listen: Some("127.0.0.1:7680".into()),
            allow_insecure_bind: false,
            trust_tailscale_self: true,
        };
        assert_eq!(
            resolve_bind_addrs(&safe_loopback).unwrap(),
            vec!["127.0.0.1:7680".parse().unwrap()]
        );

        let rejected_wg = ServerListenConfig {
            tailscale: false,
            port: 7680,
            listen: Some("10.10.0.2:7680".into()),
            allow_insecure_bind: false,
            trust_tailscale_self: true,
        };
        let wg_err = resolve_bind_addrs(&rejected_wg).unwrap_err();
        assert!(wg_err.to_string().contains("--allow-insecure-bind"));

        let rejected_unspec = ServerListenConfig {
            tailscale: false,
            port: 7680,
            listen: Some("0.0.0.0:7680".into()),
            allow_insecure_bind: false,
            trust_tailscale_self: true,
        };
        assert!(resolve_bind_addrs(&rejected_unspec).is_err());

        let allowed_wg = ServerListenConfig {
            allow_insecure_bind: true,
            ..rejected_wg
        };
        assert_eq!(
            resolve_bind_addrs(&allowed_wg).unwrap(),
            vec!["10.10.0.2:7680".parse().unwrap()]
        );

        let allowed_unspec = ServerListenConfig {
            allow_insecure_bind: true,
            ..rejected_unspec
        };
        assert_eq!(
            resolve_bind_addrs(&allowed_unspec).unwrap(),
            vec!["0.0.0.0:7680".parse().unwrap()]
        );
    }

    #[test]
    fn tailscale_whois_and_discovery_enforce_same_user_no_tags_and_skip_non_desktop_or_offline() {
        let status = r#"{
            "BackendState": "Running",
            "Self": {
                "ID": "nSelf",
                "HostName": "my-mac",
                "UserID": 42,
                "TailscaleIPs": ["100.84.0.1"]
            },
            "Peer": {
                "k1": {
                    "HostName": "linux-box",
                    "DNSName": "linux-box.tailnet.ts.net.",
                    "OS": "linux",
                    "UserID": 42,
                    "TailscaleIPs": ["100.84.0.2"],
                    "Online": true
                },
                "k2": {
                    "HostName": "my-iphone",
                    "DNSName": "my-iphone.tailnet.ts.net.",
                    "OS": "iOS",
                    "UserID": 42,
                    "TailscaleIPs": ["100.84.0.3"],
                    "Online": true
                },
                "k3": {
                    "HostName": "colleague-mac",
                    "DNSName": "colleague.tailnet.ts.net.",
                    "OS": "macOS",
                    "UserID": 99,
                    "TailscaleIPs": ["100.84.0.4"],
                    "Online": true
                },
                "k4": {
                    "HostName": "tagged-srv",
                    "DNSName": "tagged.tailnet.ts.net.",
                    "OS": "linux",
                    "UserID": 42,
                    "Tags": ["tag:server"],
                    "TailscaleIPs": ["100.84.0.5"],
                    "Online": true
                },
                "k5": {
                    "HostName": "shared-in",
                    "DNSName": "shared.tailnet.ts.net.",
                    "OS": "linux",
                    "UserID": 42,
                    "IsShareeNode": true,
                    "TailscaleIPs": ["100.84.0.6"],
                    "Online": true
                },
                "k6": {
                    "HostName": "sleepy-pc",
                    "DNSName": "sleepy.tailnet.ts.net.",
                    "OS": "windows",
                    "UserID": 42,
                    "TailscaleIPs": ["100.84.0.7"],
                    "Online": false
                },
                "k7": {
                    "HostName": "linked-via-dns",
                    "DNSName": "linked-via-dns.tailnet.ts.net.",
                    "OS": "linux",
                    "UserID": 42,
                    "TailscaleIPs": ["100.84.0.8"],
                    "Online": true
                }
            }
        }"#;
        let self_id = parse_tailscale_self_identity(status).unwrap();
        assert_eq!(self_id.user_id, 42);
        assert_eq!(self_id.stable_id, "nSelf");

        let saved = vec![SavedHost {
            id: "b".into(),
            name: "Linked DNS Host".into(),
            transport: HostTransport::Tcp {
                addr: "linked-via-dns.tailnet.ts.net:7680".into(),
                token: "dev_token".into(),
            },
            ssh_target: None,
            server_id: Some("srv-b".into()),
            enabled: true,
            added_at: 1,
        }];
        let (cands, ts_ok) = collect_canban_discovery_candidates(&saved, Some(status), "srv-self");
        assert!(ts_ok);
        let addrs: Vec<&str> = cands.iter().map(|c| c.addr.as_str()).collect();
        assert_eq!(
            addrs,
            vec!["linked-via-dns.tailnet.ts.net:7680", "100.84.0.2:7680"]
        );
        assert!(cands[0].linked_on_host);
        assert!(!cands[1].linked_on_host);

        let whois_ok = r#"{
            "Node": {
                "ID": 7,
                "StableID": "nPhone",
                "User": 42,
                "ComputedName": "iPhone 17",
                "MachineAuthorized": true
            },
            "UserProfile": { "ID": 42, "LoginName": "me@example.com" }
        }"#;
        let verified = parse_tailscale_whois_json(whois_ok, 42).expect("same user must pass");
        assert_eq!(verified.node_id, "nPhone");
        assert_eq!(verified.device_name, "iPhone 17");

        // Different UserID, tagged node, or unauthorized node must be rejected.
        assert!(parse_tailscale_whois_json(whois_ok, 99).is_none());
        let whois_tagged = r#"{
            "Node": { "StableID": "nTag", "User": 42, "Tags": ["tag:ci"] },
            "UserProfile": { "ID": 42 }
        }"#;
        assert!(parse_tailscale_whois_json(whois_tagged, 42).is_none());
    }
}
