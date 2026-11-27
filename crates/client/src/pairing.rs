//! Single-address Noise E2EE pairing URI parsing/formatting (strict `?pk=` on non-Tailscale
//! endpoints) and single-address TCP reachability probing.
use canban_transport::{decode_pubkey, pubkey_b64_fingerprint};
use canban_types::{
    PAIRING_CODE_PREFIX, extract_daemon_host, is_pairing_code, is_tailscale_addr,
    normalize_daemon_addr,
};
use std::{net::IpAddr, time::Duration};
use tokio::time::Instant;

/// Returns `true` for Tailscale CGNAT (`100.64.0.0/10`) and ULA (`fd7a:115c:a1e0::/48`) IPs.
pub fn is_tailscale_ip(ip: IpAddr) -> bool {
    canban_types::is_tailscale_ip(ip)
}

fn extract_host_part(addr: &str) -> &str {
    extract_daemon_host(addr)
}

/// Parsed single-address pairing invitation (`canban://<addr>?pk=<pubkey>[&sid=...&name=...]#pair_...`)
/// or bare Tailscale same-account auto-auth target (`host.ts.net` / `100.x.y.z`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingInvite {
    pub addr: String,
    pub server_pubkey: Option<String>,
    pub server_id: Option<String>,
    pub host_name: Option<String>,
    /// One-time `pair_...` code when pairing via QR/URI, or `None` for Tailscale WhoIs auto-auth.
    pub pairing_code: Option<String>,
    /// `true` when using Tailscale same-account WhoIs automatic sign-in.
    pub tailscale_auth: bool,
}

impl PairingInvite {
    /// Constructs a Tailscale same-account auto-auth invite from a `.ts.net` or Tailscale IP address.
    pub fn tailscale(addr: &str, host_name: Option<&str>, server_id: Option<&str>) -> Option<Self> {
        let norm = normalize_daemon_addr(addr);
        if norm.is_empty() || !is_tailscale_addr(&norm) {
            return None;
        }
        let clean_name = host_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        Some(Self {
            addr: norm,
            server_pubkey: None,
            server_id: server_id
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            host_name: clean_name,
            pairing_code: None,
            tailscale_auth: true,
        })
    }

    pub fn is_tailscale(&self) -> bool {
        is_tailscale_addr(&self.addr)
    }

    pub fn pubkey_fingerprint(&self) -> Option<String> {
        self.server_pubkey
            .as_deref()
            .and_then(pubkey_b64_fingerprint)
    }
}

/// Parses a bare Tailscale endpoint (`100.x.y.z`, `host.ts.net`, `canban://host.ts.net`,
/// or `tcp://100.x.y.z:7680` without `#pair_...`) into a Tailscale same-account `PairingInvite`.
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

/// Formats a single-address pairing URI (`canban://<addr>?pk=<pubkey>[&sid=...&name=...]#<code_>`).
pub fn format_pairing_uri(
    addr: &str,
    server_pubkey: &str,
    code: &str,
    server_id: Option<&str>,
    host_name: Option<&str>,
) -> String {
    let norm_addr = normalize_daemon_addr(addr);
    let mut query = Vec::new();
    let pk = server_pubkey.trim();
    if !pk.is_empty() {
        query.push(format!("pk={}", percent_encode(pk)));
    }
    if let Some(sid) = server_id.map(str::trim).filter(|s| !s.is_empty()) {
        query.push(format!("sid={}", percent_encode(sid)));
    }
    if let Some(name) = host_name.map(str::trim).filter(|s| !s.is_empty()) {
        query.push(format!("name={}", percent_encode(name)));
    }
    if query.is_empty() {
        format!("canban://{norm_addr}#{}", code.trim())
    } else {
        format!("canban://{norm_addr}?{}#{}", query.join("&"), code.trim())
    }
}

/// Parses a single-address pairing URI (`canban://<addr>?pk=<pubkey>[&sid=...&name=...]#pair_...`).
///
/// Enforces strict host-key pinning: non-Tailscale endpoints MUST carry a valid 32-byte `?pk=`
/// Noise X25519 public key.
pub fn parse_pairing_invite(raw: &str) -> Option<PairingInvite> {
    let trimmed = raw.trim();
    let (before_frag, frag) = trimmed.split_once('#')?;
    let code = frag.trim();
    if !is_pairing_code(code) || code.len() < PAIRING_CODE_PREFIX.len() + 16 {
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

    let mut server_pubkey = None;
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
                "pk" => {
                    let pk = decoded.trim();
                    if decode_pubkey(pk).is_err() {
                        return None;
                    }
                    server_pubkey = Some(pk.to_owned());
                }
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
    if !is_tailscale_addr(&addr) && server_pubkey.is_none() {
        return None;
    }
    Some(PairingInvite {
        addr,
        server_pubkey,
        server_id,
        host_name,
        pairing_code: Some(code.to_owned()),
        tailscale_auth: false,
    })
}

/// Reachability and RTT result for the single address in a pairing invitation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddrProbeResult {
    pub addr: String,
    pub is_tailscale: bool,
    pub reachable: bool,
    pub rtt_ms: Option<u64>,
    pub error: Option<String>,
}

/// Probes TCP reachability of a single address without consuming single-use pairing codes.
pub async fn probe_addr(addr: &str, timeout: Duration) -> AddrProbeResult {
    let norm = normalize_daemon_addr(addr);
    let is_tailscale = is_tailscale_addr(&norm);
    if norm.is_empty() {
        return AddrProbeResult {
            addr: norm,
            is_tailscale,
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
                is_tailscale,
                reachable: true,
                rtt_ms: Some(rtt_ms),
                error: None,
            }
        }
        Ok(Err(e)) => AddrProbeResult {
            addr: norm,
            is_tailscale,
            reachable: false,
            rtt_ms: None,
            error: Some(e.to_string()),
        },
        Err(_) => AddrProbeResult {
            addr: norm,
            is_tailscale,
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
            is_tailscale: is_tailscale_addr(addr),
            reachable: false,
            rtt_ms: None,
            error: Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_transport::NoiseKeypair;

    #[test]
    fn single_address_pairing_uri_round_trip() {
        let code = "pair_0123456789abcdef0123456789abcdef";
        let pk = NoiseKeypair::generate().unwrap().public_b64();

        let with_meta = format_pairing_uri(
            "home-mac.tailnet.ts.net",
            &pk,
            code,
            Some("srv-1"),
            Some("Home Mac #1"),
        );
        assert_eq!(
            with_meta,
            format!(
                "canban://home-mac.tailnet.ts.net:7680?pk={pk}&sid=srv-1&name=Home%20Mac%20%231#{code}"
            )
        );
        let parsed = parse_pairing_invite(&with_meta).expect("valid URI with metadata");
        assert_eq!(parsed.addr, "home-mac.tailnet.ts.net:7680");
        assert_eq!(parsed.server_pubkey.as_deref(), Some(pk.as_str()));
        assert_eq!(parsed.server_id.as_deref(), Some("srv-1"));
        assert_eq!(parsed.host_name.as_deref(), Some("Home Mac #1"));
        assert_eq!(parsed.pairing_code.as_deref(), Some(code));
        assert!(!parsed.tailscale_auth);

        // Non-Tailscale URI with `?pk=` parses cleanly; without `?pk=` is strictly rejected.
        let lan_with_pk = format_pairing_uri("192.168.1.20:7680", &pk, code, None, None);
        assert_eq!(
            lan_with_pk,
            format!("canban://192.168.1.20:7680?pk={pk}#{code}")
        );
        let parsed_lan = parse_pairing_invite(&lan_with_pk).expect("valid LAN URI with pk");
        assert_eq!(parsed_lan.addr, "192.168.1.20:7680");
        assert_eq!(parsed_lan.server_pubkey.as_deref(), Some(pk.as_str()));
        assert!(parse_pairing_invite(&format!("canban://192.168.1.20:7680#{code}")).is_none());

        let tcp_raw = format!("tcp://100.84.12.5#{code}");
        let parsed_tcp = parse_pairing_invite(&tcp_raw).expect("tailscale URI without pk");
        assert_eq!(parsed_tcp.addr, "100.84.12.5:7680");
        assert!(parsed_tcp.is_tailscale());

        assert!(parse_pairing_invite("canban://127.0.0.1:7680#short").is_none());
        assert!(
            parse_pairing_invite(&format!("canban://pair?v=2&a=100.64.1.2:7680#{code}")).is_none()
        );
    }

    #[test]
    fn parse_tailscale_host_input_accepts_tailscale_endpoints_and_rejects_lan_or_secrets() {
        let ts_ip = parse_tailscale_host_input("100.84.12.5").expect("tailscale ipv4");
        assert_eq!(ts_ip.addr, "100.84.12.5:7680");
        assert!(ts_ip.tailscale_auth);
        assert!(ts_ip.pairing_code.is_none());
        assert_eq!(ts_ip.host_name, None);

        let ts_dns = parse_tailscale_host_input("canban://home-linux.tailnet-abc.ts.net:7680")
            .expect("magicdns");
        assert_eq!(ts_dns.addr, "home-linux.tailnet-abc.ts.net:7680");
        assert_eq!(ts_dns.host_name.as_deref(), Some("home-linux"));
        assert!(ts_dns.tailscale_auth);

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
        assert!(!open_res.is_tailscale);

        let closed_res = probe_addr(&closed_addr, Duration::from_secs(2)).await;
        assert!(!closed_res.reachable);
        assert_eq!(closed_res.rtt_ms, None);
    }
}
