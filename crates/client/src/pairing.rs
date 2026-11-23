//! Pairing URI v1/v2 parsing/formatting, address security classification,
//! and concurrent TCP candidate probing.
use canban_types::{normalize_daemon_addr, parse_pairing_uri};
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};

/// Network security classification displayed during QR pairing and host inspection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressSecurity {
    Tailscale,
    Other,
}

/// Returns `true` for Tailscale CGNAT (`100.64.0.0/10`) and ULA (`fd7a:115c:a1e0::/48`) IPs.
pub fn is_tailscale_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 100 && (octets[1] & 0b1100_0000) == 64
        }
        IpAddr::V6(v6) => {
            let segs = v6.segments();
            segs[0] == 0xfd7a && segs[1] == 0x115c && segs[2] == 0xa1e0
        }
    }
}

fn extract_host_part(addr: &str) -> &str {
    let trimmed = addr
        .trim()
        .strip_prefix("tcp://")
        .or_else(|| addr.trim().strip_prefix("canban://"))
        .unwrap_or(addr.trim())
        .trim_end_matches('/');
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some((host, _)) = rest.split_once(']')
    {
        return host;
    }
    if let Some((host, port_str)) = trimmed.rsplit_once(':')
        && !host.contains(':')
        && port_str.parse::<u16>().is_ok()
    {
        return host;
    }
    trimmed
}

impl AddressSecurity {
    /// Classifies a candidate address or `host:port` into Tailscale vs non-Tailscale.
    pub fn classify(addr: &str) -> Self {
        let host = extract_host_part(addr);
        if host.eq_ignore_ascii_case("ts.net") || host.to_ascii_lowercase().ends_with(".ts.net") {
            return Self::Tailscale;
        }
        if let Ok(ip) = host.parse::<IpAddr>()
            && is_tailscale_ip(ip)
        {
            return Self::Tailscale;
        }
        Self::Other
    }
}

/// Lower value means higher preference (`0` = Tailscale MagicDNS, `1` = Tailscale IP, `2` = other).
pub fn address_priority(addr: &str) -> u8 {
    let host = extract_host_part(addr);
    if host.to_ascii_lowercase().ends_with(".ts.net") {
        0
    } else if host.parse::<IpAddr>().is_ok_and(is_tailscale_ip) {
        1
    } else {
        2
    }
}

/// Parsed pairing invitation (URI v2 multi-candidate or legacy v1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingInvite {
    pub version: u8,
    pub server_id: Option<String>,
    pub host_name: Option<String>,
    pub addresses: Vec<String>,
    pub secret: String,
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

fn normalize_and_sort_addrs(raw: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for item in raw {
        let norm = normalize_daemon_addr(&item);
        if !norm.is_empty() && !deduped.contains(&norm) {
            deduped.push(norm);
        }
    }
    deduped.sort_by_key(|a| address_priority(a));
    deduped
}

/// Formats a v2 pairing URI (`canban://pair?v=2&sid=...&name=...&a=...#<secret>`).
pub fn format_pairing_uri_v2(
    server_id: Option<&str>,
    host_name: Option<&str>,
    addresses: &[String],
    secret: &str,
) -> String {
    let mut query = vec!["v=2".to_owned()];
    if let Some(sid) = server_id.map(str::trim).filter(|s| !s.is_empty()) {
        query.push(format!("sid={}", percent_encode(sid)));
    }
    if let Some(name) = host_name.map(str::trim).filter(|s| !s.is_empty()) {
        query.push(format!("name={}", percent_encode(name)));
    }
    let ordered = normalize_and_sort_addrs(addresses.to_vec());
    for addr in ordered {
        query.push(format!("a={}", percent_encode(&addr)));
    }
    format!("canban://pair?{}#{}", query.join("&"), secret.trim())
}

/// Parses either a v2 (`canban://pair?v=2&...#<secret>`) or legacy v1 (`canban://<addr>#<token>`) URI.
pub fn parse_pairing_invite(raw: &str) -> Option<PairingInvite> {
    let trimmed = raw.trim();
    if let Some((before_frag, frag)) = trimmed.split_once('#')
        && let Some(after_scheme) = before_frag
            .strip_prefix("canban://")
            .or_else(|| before_frag.strip_prefix("tcp://"))
        && let Some((host_or_route, query)) = after_scheme.split_once('?')
        && host_or_route.trim_end_matches('/') == "pair"
    {
        let secret = frag.trim();
        if secret.len() < 16 {
            return None;
        }
        let mut version = 2u8;
        let mut server_id = None;
        let mut host_name = None;
        let mut addrs = Vec::new();
        for pair in query.split('&') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            let decoded = percent_decode(v)?;
            match k {
                "v" => {
                    if let Ok(parsed) = decoded.trim().parse::<u8>() {
                        version = parsed;
                    }
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
                "a" => {
                    let s = decoded.trim();
                    if !s.is_empty() {
                        addrs.push(s.to_owned());
                    }
                }
                _ => {}
            }
        }
        if version != 2 {
            return None;
        }
        let addresses = normalize_and_sort_addrs(addrs);
        if addresses.is_empty() {
            return None;
        }
        return Some(PairingInvite {
            version,
            server_id,
            host_name,
            addresses,
            secret: secret.to_owned(),
        });
    }

    let (addr, secret) = parse_pairing_uri(trimmed)?;
    Some(PairingInvite {
        version: 1,
        server_id: None,
        host_name: None,
        addresses: vec![addr],
        secret,
    })
}

/// Reachability + RTT result for a single pairing candidate address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateProbeResult {
    pub addr: String,
    pub security: AddressSecurity,
    pub reachable: bool,
    pub rtt_ms: Option<u64>,
    pub error: Option<String>,
}

/// Concurrently probes TCP reachability of all candidate addresses without consuming single-use
/// pairing codes. Results are ordered with reachable Tailscale MagicDNS > Tailscale IP > other.
pub async fn probe_candidates(
    addresses: &[String],
    timeout: Duration,
) -> Vec<CandidateProbeResult> {
    let ordered = normalize_and_sort_addrs(addresses.to_vec());
    let mut set = tokio::task::JoinSet::new();
    for addr in ordered {
        set.spawn(async move {
            let security = AddressSecurity::classify(&addr);
            let start = Instant::now();
            match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr.as_str())).await
            {
                Ok(Ok(stream)) => {
                    let rtt_ms = start.elapsed().as_millis() as u64;
                    drop(stream);
                    CandidateProbeResult {
                        addr,
                        security,
                        reachable: true,
                        rtt_ms: Some(rtt_ms),
                        error: None,
                    }
                }
                Ok(Err(e)) => CandidateProbeResult {
                    addr,
                    security,
                    reachable: false,
                    rtt_ms: None,
                    error: Some(e.to_string()),
                },
                Err(_) => CandidateProbeResult {
                    addr,
                    security,
                    reachable: false,
                    rtt_ms: None,
                    error: Some("Timed out".into()),
                },
            }
        });
    }

    let mut results = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(item) = joined {
            results.push(item);
        }
    }
    results.sort_by_key(|r| {
        (
            !r.reachable,
            address_priority(&r.addr),
            r.rtt_ms.unwrap_or(u64::MAX),
            r.addr.clone(),
        )
    });
    results
}

/// Returns the highest-priority reachable candidate from `probe_candidates`.
pub fn select_preferred_candidate(
    results: &[CandidateProbeResult],
) -> Option<&CandidateProbeResult> {
    results.iter().find(|r| r.reachable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_security_and_priority_classify_tailscale_vs_other() {
        assert_eq!(
            AddressSecurity::classify("home-mac.tailnet-abc.ts.net:7680"),
            AddressSecurity::Tailscale
        );
        assert_eq!(address_priority("home-mac.tailnet-abc.ts.net:7680"), 0);

        assert_eq!(
            AddressSecurity::classify("100.84.12.5:7680"),
            AddressSecurity::Tailscale
        );
        assert_eq!(address_priority("100.84.12.5:7680"), 1);

        assert_eq!(
            AddressSecurity::classify("[fd7a:115c:a1e0::1]:7680"),
            AddressSecurity::Tailscale
        );
        assert_eq!(address_priority("[fd7a:115c:a1e0::1]:7680"), 1);

        assert_eq!(
            AddressSecurity::classify("192.168.1.20:7680"),
            AddressSecurity::Other
        );
        assert_eq!(address_priority("192.168.1.20:7680"), 2);
        assert_eq!(
            AddressSecurity::classify("100.128.0.1:7680"),
            AddressSecurity::Other
        );
    }

    #[test]
    fn pairing_invite_v2_and_v1_round_trip_and_order_candidates() {
        let secret = "pair_0123456789abcdef0123456789abcdef";
        let addrs = vec![
            "192.168.1.20:7680".to_owned(),
            "100.64.1.2:7680".to_owned(),
            "home-mac.tailnet.ts.net".to_owned(),
            "100.64.1.2:7680".to_owned(),
        ];
        let uri = format_pairing_uri_v2(Some("srv-1"), Some("Home Mac #1"), &addrs, secret);
        assert!(uri.starts_with("canban://pair?v=2&sid=srv-1&name=Home%20Mac%20%231&"));
        assert!(uri.ends_with(&format!("#{secret}")));

        let parsed = parse_pairing_invite(&uri).expect("valid v2 URI");
        assert_eq!(parsed.version, 2);
        assert_eq!(parsed.server_id.as_deref(), Some("srv-1"));
        assert_eq!(parsed.host_name.as_deref(), Some("Home Mac #1"));
        assert_eq!(parsed.secret, secret);
        assert_eq!(
            parsed.addresses,
            vec![
                "home-mac.tailnet.ts.net:7680",
                "100.64.1.2:7680",
                "192.168.1.20:7680",
            ]
        );

        let v1_uri = format!("canban://100.84.12.5:7680#{secret}");
        let v1 = parse_pairing_invite(&v1_uri).expect("valid v1 URI");
        assert_eq!(v1.version, 1);
        assert_eq!(v1.addresses, vec!["100.84.12.5:7680"]);
        assert_eq!(v1.secret, secret);

        assert!(parse_pairing_invite("canban://pair?v=2&a=127.0.0.1:7680#short").is_none());
        assert!(parse_pairing_invite("canban://pair?v=2#0123456789abcdef0123").is_none());
    }

    #[tokio::test]
    async fn probe_candidates_measures_reachability_and_prefers_priority_order() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open_addr = listener.local_addr().unwrap().to_string();

        let closed_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_addr = closed_listener.local_addr().unwrap().to_string();
        drop(closed_listener);

        let _accept = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let results = probe_candidates(
            &[closed_addr.clone(), open_addr.clone()],
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(results.len(), 2);
        let preferred = select_preferred_candidate(&results).expect("open port reachable");
        assert_eq!(preferred.addr, open_addr);
        assert!(preferred.reachable);
        assert!(preferred.rtt_ms.is_some());
        assert!(!results[1].reachable);
        assert_eq!(results[1].addr, closed_addr);
    }
}
