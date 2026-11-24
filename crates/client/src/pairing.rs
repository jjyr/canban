//! Single-address pairing URI parsing/formatting, transport security classification
//! (including Tailscale & SSH), and single-address TCP reachability probing.
use canban_types::{
    HostTransport, TAILSCALE_AUTH_SENTINEL, extract_daemon_host, normalize_daemon_addr,
};
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};

/// Transport confidentiality classification displayed during pairing and host inspection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportSecurity {
    Local,
    Tailscale,
    Ssh,
    UnverifiedTcp,
}

/// Returns `true` for Tailscale CGNAT (`100.64.0.0/10`) and ULA (`fd7a:115c:a1e0::/48`) IPs.
pub fn is_tailscale_ip(ip: IpAddr) -> bool {
    canban_types::is_tailscale_ip(ip)
}

fn extract_host_part(addr: &str) -> &str {
    extract_daemon_host(addr)
}

impl TransportSecurity {
    /// Classifies a TCP `host:port` endpoint into Tailscale vs unverified direct TCP.
    pub fn classify_tcp_addr(addr: &str) -> Self {
        let host = extract_host_part(addr);
        if host.eq_ignore_ascii_case("ts.net") || host.to_ascii_lowercase().ends_with(".ts.net") {
            return Self::Tailscale;
        }
        if let Ok(ip) = host.parse::<IpAddr>()
            && is_tailscale_ip(ip)
        {
            return Self::Tailscale;
        }
        Self::UnverifiedTcp
    }

    /// Classifies a saved [`HostTransport`] for UI security icons and hints.
    pub fn classify_transport(transport: &HostTransport) -> Self {
        match transport {
            HostTransport::Local => Self::Local,
            HostTransport::Ssh { .. } => Self::Ssh,
            HostTransport::Tcp { addr, .. } => Self::classify_tcp_addr(addr),
        }
    }

    /// Returns `true` when the underlying link provides transport encryption (`Local`, `Tailscale`, or `Ssh`).
    pub fn is_encrypted(self) -> bool {
        matches!(self, Self::Local | Self::Tailscale | Self::Ssh)
    }

    /// Concise transport label for pairing confirmation and host rows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "Local socket",
            Self::Tailscale => "Tailscale",
            Self::Ssh => "SSH",
            Self::UnverifiedTcp => "Not Tailscale",
        }
    }

    /// User-facing transport security hint matching mobile pairing and host UI guidelines.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Local => "Local Unix socket",
            Self::Tailscale => "Tailscale — encrypted by WireGuard",
            Self::Ssh => "SSH — encrypted tunnel",
            Self::UnverifiedTcp => {
                "Not Tailscale — may be unencrypted unless your own VPN protects it"
            }
        }
    }
}

/// Parsed single-address pairing invitation (`canban://<addr>[?sid=...&name=...]#<secret>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingInvite {
    pub addr: String,
    pub security: TransportSecurity,
    pub server_id: Option<String>,
    pub host_name: Option<String>,
    pub secret: String,
}

impl PairingInvite {
    /// Constructs a Tailscale same-account auto-auth invite from a `.ts.net` or Tailscale IP address.
    pub fn tailscale(addr: &str, host_name: Option<&str>, server_id: Option<&str>) -> Option<Self> {
        let norm = normalize_daemon_addr(addr);
        if norm.is_empty() {
            return None;
        }
        let security = TransportSecurity::classify_tcp_addr(&norm);
        if security != TransportSecurity::Tailscale {
            return None;
        }
        let clean_name = host_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        Some(Self {
            addr: norm,
            security,
            server_id: server_id
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            host_name: clean_name,
            secret: TAILSCALE_AUTH_SENTINEL.to_owned(),
        })
    }
}

/// Parses a bare Tailscale endpoint (`100.x.y.z`, `host.ts.net`, `canban://host.ts.net`,
/// or `tcp://100.x.y.z:7680` without `#secret`) into a Tailscale same-account `PairingInvite`.
pub fn parse_tailscale_host_input(raw: &str) -> Option<PairingInvite> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('#') || trimmed.contains('?') {
        return None;
    }
    let without_scheme = trimmed
        .strip_prefix("canban://")
        .or_else(|| trimmed.strip_prefix("tcp://"))
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    if without_scheme.is_empty() || without_scheme.contains(char::is_whitespace) {
        return None;
    }
    let host_part = extract_host_part(without_scheme);
    let derived_name = host_part
        .split('.')
        .next()
        .filter(|s| !s.is_empty() && host_part.parse::<IpAddr>().is_err());
    PairingInvite::tailscale(without_scheme, derived_name, None)
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Formats a single-address pairing URI (`canban://<addr>[?sid=...&name=...]#<secret>`).
pub fn format_pairing_uri(
    addr: &str,
    secret: &str,
    server_id: Option<&str>,
    host_name: Option<&str>,
) -> String {
    let norm_addr = normalize_daemon_addr(addr);
    let mut query = Vec::new();
    if let Some(sid) = server_id.map(str::trim).filter(|s| !s.is_empty()) {
        query.push(format!("sid={}", percent_encode(sid)));
    }
    if let Some(name) = host_name.map(str::trim).filter(|s| !s.is_empty()) {
        query.push(format!("name={}", percent_encode(name)));
    }
    if query.is_empty() {
        format!("canban://{norm_addr}#{}", secret.trim())
    } else {
        format!("canban://{norm_addr}?{}#{}", query.join("&"), secret.trim())
    }
}

/// Parses a single-address pairing URI (`canban://<addr>[?sid=...&name=...]#<secret>`,
/// `tcp://<addr>#<secret>`, or `<addr>#<secret>`). Rejects legacy multi-candidate `?v=2&a=...` URIs.
pub fn parse_pairing_invite(raw: &str) -> Option<PairingInvite> {
    let trimmed = raw.trim();
    let (before_frag, frag) = trimmed.split_once('#')?;
    let secret = frag.trim();
    if secret.len() < 16 {
        return None;
    }

    let without_scheme = before_frag
        .strip_prefix("canban://")
        .or_else(|| before_frag.strip_prefix("tcp://"))
        .unwrap_or(before_frag);

    let (raw_host, raw_query) = match without_scheme.split_once('?') {
        Some((h, q)) => (h.trim().trim_end_matches('/'), Some(q)),
        None => (without_scheme.trim().trim_end_matches('/'), None),
    };
    if raw_host.is_empty() || raw_host.eq_ignore_ascii_case("pair") {
        return None;
    }

    let mut server_id = None;
    let mut host_name = None;
    if let Some(query) = raw_query {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=')?;
            let decoded = percent_decode(v)?;
            match k {
                "sid" => {
                    let s = decoded.trim();
                    if !s.is_empty() {
                        server_id = Some(s.to_owned());
                    }
                }
                "name" => {
                    let s = decoded.trim();
                    if !s.is_empty() {
                        host_name = Some(s.to_owned());
                    }
                }
                "v" | "a" => return None,
                _ => {}
            }
        }
    }

    let addr = normalize_daemon_addr(raw_host);
    if addr.is_empty() {
        return None;
    }
    let security = TransportSecurity::classify_tcp_addr(&addr);
    Some(PairingInvite {
        addr,
        security,
        server_id,
        host_name,
        secret: secret.to_owned(),
    })
}

/// Reachability and RTT result for the single address in a pairing invitation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddrProbeResult {
    pub addr: String,
    pub security: TransportSecurity,
    pub reachable: bool,
    pub rtt_ms: Option<u64>,
    pub error: Option<String>,
}

/// Probes TCP reachability of a single address without consuming single-use pairing codes.
pub async fn probe_addr(addr: &str, timeout: Duration) -> AddrProbeResult {
    let norm = normalize_daemon_addr(addr);
    let security = TransportSecurity::classify_tcp_addr(&norm);
    if norm.is_empty() {
        return AddrProbeResult {
            addr: norm,
            security,
            reachable: false,
            rtt_ms: None,
            error: Some("Empty address".into()),
        };
    }
    let start = Instant::now();
    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(norm.as_str())).await {
        Ok(Ok(stream)) => {
            let rtt_ms = start.elapsed().as_millis() as u64;
            drop(stream);
            AddrProbeResult {
                addr: norm,
                security,
                reachable: true,
                rtt_ms: Some(rtt_ms),
                error: None,
            }
        }
        Ok(Err(e)) => AddrProbeResult {
            addr: norm,
            security,
            reachable: false,
            rtt_ms: None,
            error: Some(e.to_string()),
        },
        Err(_) => AddrProbeResult {
            addr: norm,
            security,
            reachable: false,
            rtt_ms: None,
            error: Some("Timed out".into()),
        },
    }
}

/// Blocking wrapper around [`probe_addr`] suitable for UI `blocking(...)` thread-pool calls.
pub fn probe_addr_blocking(addr: &str, timeout: Duration) -> AddrProbeResult {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return handle.block_on(probe_addr(addr, timeout));
    }
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt.block_on(probe_addr(addr, timeout)),
        Err(e) => AddrProbeResult {
            addr: normalize_daemon_addr(addr),
            security: TransportSecurity::classify_tcp_addr(addr),
            reachable: false,
            rtt_ms: None,
            error: Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_security_classifies_local_tailscale_ssh_and_unverified_tcp() {
        assert_eq!(
            TransportSecurity::classify_tcp_addr("home-mac.tailnet-abc.ts.net:7680"),
            TransportSecurity::Tailscale
        );
        assert_eq!(
            TransportSecurity::classify_tcp_addr("100.84.12.5:7680"),
            TransportSecurity::Tailscale
        );
        assert_eq!(
            TransportSecurity::classify_tcp_addr("[fd7a:115c:a1e0::1]:7680"),
            TransportSecurity::Tailscale
        );
        assert_eq!(
            TransportSecurity::classify_tcp_addr("192.168.1.20:7680"),
            TransportSecurity::UnverifiedTcp
        );
        assert_eq!(
            TransportSecurity::classify_tcp_addr("100.128.0.1:7680"),
            TransportSecurity::UnverifiedTcp
        );

        let local = TransportSecurity::classify_transport(&HostTransport::Local);
        assert_eq!(local, TransportSecurity::Local);
        assert!(local.is_encrypted());
        assert_eq!(local.label(), "Local socket");

        let ssh = TransportSecurity::classify_transport(&HostTransport::Ssh {
            target: "jjy@home-mac".into(),
            remote_home: None,
        });
        assert_eq!(ssh, TransportSecurity::Ssh);
        assert!(ssh.is_encrypted());
        assert_eq!(ssh.label(), "SSH");
        assert!(ssh.hint().contains("encrypted tunnel"));

        let ts = TransportSecurity::classify_transport(&HostTransport::Tcp {
            addr: "home-mac.ts.net:7680".into(),
            token: "dev_token".into(),
        });
        assert_eq!(ts, TransportSecurity::Tailscale);
        assert!(ts.is_encrypted());
        assert!(ts.hint().contains("WireGuard"));

        let lan = TransportSecurity::classify_transport(&HostTransport::Tcp {
            addr: "192.168.1.20:7680".into(),
            token: "dev_token".into(),
        });
        assert_eq!(lan, TransportSecurity::UnverifiedTcp);
        assert!(!lan.is_encrypted());
        assert!(lan.hint().contains("Not Tailscale"));
    }

    #[test]
    fn single_address_pairing_uri_round_trip() {
        let secret = "pair_0123456789abcdef0123456789abcdef";

        let with_meta = format_pairing_uri(
            "home-mac.tailnet.ts.net",
            secret,
            Some("srv-1"),
            Some("Home Mac #1"),
        );
        assert_eq!(
            with_meta,
            format!(
                "canban://home-mac.tailnet.ts.net:7680?sid=srv-1&name=Home%20Mac%20%231#{secret}"
            )
        );
        let parsed = parse_pairing_invite(&with_meta).expect("valid URI with metadata");
        assert_eq!(parsed.addr, "home-mac.tailnet.ts.net:7680");
        assert_eq!(parsed.security, TransportSecurity::Tailscale);
        assert_eq!(parsed.server_id.as_deref(), Some("srv-1"));
        assert_eq!(parsed.host_name.as_deref(), Some("Home Mac #1"));
        assert_eq!(parsed.secret, secret);

        let plain = format_pairing_uri("192.168.1.20:7680", secret, None, None);
        assert_eq!(plain, format!("canban://192.168.1.20:7680#{secret}"));
        let parsed_plain = parse_pairing_invite(&plain).expect("valid plain URI");
        assert_eq!(parsed_plain.addr, "192.168.1.20:7680");
        assert_eq!(parsed_plain.security, TransportSecurity::UnverifiedTcp);
        assert_eq!(parsed_plain.server_id, None);
        assert_eq!(parsed_plain.host_name, None);

        let tcp_raw = format!("tcp://100.84.12.5#{secret}");
        let parsed_tcp = parse_pairing_invite(&tcp_raw).expect("tcp URI without port");
        assert_eq!(parsed_tcp.addr, "100.84.12.5:7680");
        assert_eq!(parsed_tcp.security, TransportSecurity::Tailscale);

        assert!(parse_pairing_invite("canban://127.0.0.1:7680#short").is_none());
        assert!(
            parse_pairing_invite(&format!("canban://pair?v=2&a=100.64.1.2:7680#{secret}"))
                .is_none()
        );
    }

    #[test]
    fn parse_tailscale_host_input_accepts_tailscale_endpoints_and_rejects_lan_or_secrets() {
        let ts_ip = parse_tailscale_host_input("100.84.12.5").expect("tailscale ipv4");
        assert_eq!(ts_ip.addr, "100.84.12.5:7680");
        assert_eq!(ts_ip.security, TransportSecurity::Tailscale);
        assert_eq!(ts_ip.secret, TAILSCALE_AUTH_SENTINEL);
        assert_eq!(ts_ip.host_name, None);

        let ts_dns = parse_tailscale_host_input("canban://home-linux.tailnet-abc.ts.net:7680")
            .expect("magicdns");
        assert_eq!(ts_dns.addr, "home-linux.tailnet-abc.ts.net:7680");
        assert_eq!(ts_dns.host_name.as_deref(), Some("home-linux"));
        assert_eq!(ts_dns.secret, TAILSCALE_AUTH_SENTINEL);

        let ts_v6 = parse_tailscale_host_input("fd7a:115c:a1e0::1").expect("tailscale ipv6");
        assert_eq!(ts_v6.addr, "[fd7a:115c:a1e0::1]:7680");

        assert!(parse_tailscale_host_input("192.168.1.20").is_none());
        assert!(parse_tailscale_host_input("example.com").is_none());
        assert!(parse_tailscale_host_input("100.84.12.5:7680#pair_0123456789abcdef").is_none());
    }

    #[tokio::test]
    async fn probe_addr_reports_reachable_and_unreachable_states() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open_addr = listener.local_addr().unwrap().to_string();

        let closed_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_addr = closed_listener.local_addr().unwrap().to_string();
        drop(closed_listener);

        let _accept = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let open_res = probe_addr(&open_addr, Duration::from_secs(2)).await;
        assert!(open_res.reachable);
        assert!(open_res.rtt_ms.is_some());
        assert_eq!(open_res.security, TransportSecurity::UnverifiedTcp);

        let closed_res = probe_addr(&closed_addr, Duration::from_secs(2)).await;
        assert!(!closed_res.reachable);
        assert_eq!(closed_res.rtt_ms, None);
    }
}
