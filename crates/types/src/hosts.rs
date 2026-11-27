use super::*;

pub const DEFAULT_DAEMON_PORT: u16 = 7680;

fn default_daemon_port() -> u16 {
    DEFAULT_DAEMON_PORT
}

fn default_enabled() -> bool {
    true
}

fn default_trust_tailscale_self() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerListenConfig {
    #[serde(default)]
    pub tailscale: bool,
    #[serde(default = "default_daemon_port")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    #[serde(default = "default_trust_tailscale_self")]
    pub trust_tailscale_self: bool,
}

impl Default for ServerListenConfig {
    fn default() -> Self {
        Self {
            tailscale: false,
            port: DEFAULT_DAEMON_PORT,
            listen: None,
            trust_tailscale_self: true,
        }
    }
}

pub const PAIRING_CODE_PREFIX: &str = "pair_";

pub fn is_pairing_code(token: &str) -> bool {
    token.trim().starts_with(PAIRING_CODE_PREFIX)
}

/// Validates syntactic form of a 32-byte unpadded base64url Noise X25519 public key (43 chars).
pub fn is_valid_noise_pubkey_b64(raw: &str) -> bool {
    let trimmed = raw.trim();
    trimmed.len() == 43
        && trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pubkey_fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveredCanbanHost {
    pub name: String,
    pub addr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub rtt_ms: u64,
    #[serde(default)]
    pub linked_on_host: bool,
    #[serde(default)]
    pub is_tailscale: bool,
}

impl ServerListenConfig {
    pub fn tcp_enabled(&self) -> bool {
        self.tailscale || self.listen.as_deref().is_some_and(|s| !s.trim().is_empty())
    }

    pub fn trust_tailscale_self_enabled(&self) -> bool {
        self.trust_tailscale_self && self.tcp_enabled()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostTransport {
    Local,
    Tcp {
        addr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_pubkey: Option<String>,
        #[serde(default)]
        tailscale_auth: bool,
    },
    Ssh {
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_home: Option<String>,
    },
}

impl HostTransport {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Local => "Local socket",
            Self::Tcp { .. } => "Tailscale / TCP",
            Self::Ssh { .. } => "SSH tunnel",
        }
    }

    pub fn endpoint_display(&self) -> String {
        match self {
            Self::Local => "~/.canban/app.sock".into(),
            Self::Tcp { addr, .. } => addr.clone(),
            Self::Ssh {
                target,
                remote_home: Some(h),
            } => format!("{target}:{h}"),
            Self::Ssh {
                target,
                remote_home: None,
            } => target.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedHost {
    pub id: String,
    pub name: String,
    pub transport: HostTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub added_at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostsFile {
    #[serde(default)]
    pub hosts: Vec<SavedHost>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HostConnectionState {
    #[default]
    Connecting,
    Connected {
        #[serde(default)]
        rtt_ms: u64,
        #[serde(default)]
        api_version: u32,
        #[serde(default)]
        version: String,
        #[serde(default)]
        platform: String,
    },
    Updating {
        #[serde(default)]
        version: String,
    },
    Reconnecting {
        #[serde(default)]
        attempt: u32,
        #[serde(default)]
        error: String,
    },
    ApiMismatch {
        server_api: u32,
        app_api: u32,
        #[serde(default)]
        version: String,
    },
    AuthFailed {
        error: String,
    },
    KeyMismatch {
        #[serde(default)]
        expected_fingerprint: String,
        #[serde(default)]
        actual_fingerprint: String,
        error: String,
    },
    Offline {
        error: String,
        #[serde(default)]
        cached_at: Option<u64>,
    },
}

impl HostConnectionState {
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }

    pub fn is_updating(&self) -> bool {
        matches!(self, Self::Updating { .. })
    }

    pub fn short_status(&self) -> String {
        match self {
            Self::Connecting => "Connecting…".into(),
            Self::Connected { rtt_ms, .. } => {
                if *rtt_ms == 0 {
                    "Connected".into()
                } else {
                    format!("Connected · {rtt_ms}ms")
                }
            }
            Self::Updating { version } => {
                let ver = version.trim();
                if ver.is_empty() {
                    "Updating daemon…".into()
                } else {
                    format!("Updating to {ver}…")
                }
            }
            Self::Reconnecting { attempt, .. } => {
                if *attempt <= 1 {
                    "Reconnecting…".into()
                } else {
                    format!("Reconnecting (#{attempt})…")
                }
            }
            Self::ApiMismatch {
                server_api,
                app_api,
                ..
            } => format!("API mismatch (v{server_api} vs App v{app_api})"),
            Self::AuthFailed { .. } => "Authentication required".into(),
            Self::KeyMismatch { .. } => "Host key changed".into(),
            Self::Offline {
                cached_at: Some(_), ..
            } => "Offline · Cached".into(),
            Self::Offline {
                cached_at: None, ..
            } => "Offline".into(),
        }
    }

    pub fn error_detail(&self) -> Option<&str> {
        match self {
            Self::Reconnecting { error, .. }
            | Self::AuthFailed { error }
            | Self::KeyMismatch { error, .. }
            | Self::Offline { error, .. }
                if !error.is_empty() =>
            {
                Some(error.as_str())
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSummary {
    pub id: String,
    pub name: String,
    pub is_local: bool,
    pub transport_label: String,
    pub endpoint_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_pubkey_fingerprint: Option<String>,
    pub connection: HostConnectionState,
    #[serde(default)]
    pub active_agents: usize,
    #[serde(default)]
    pub max_agents: usize,
    #[serde(default)]
    pub project_count: usize,
    #[serde(default)]
    pub reconnect_count: u32,
    #[serde(default)]
    pub last_synced_at: Option<u64>,
    #[serde(default)]
    pub pending_mutations: usize,
    #[serde(default)]
    pub sync_conflicts: usize,
}

impl HostSummary {
    pub fn is_connected(&self) -> bool {
        self.is_local || self.connection.is_connected()
    }

    pub fn is_updating(&self) -> bool {
        !self.is_local && self.connection.is_updating()
    }

    pub fn is_offline(&self) -> bool {
        !self.is_connected()
    }

    pub fn needs_attention(&self) -> bool {
        matches!(
            self.connection,
            HostConnectionState::ApiMismatch { .. }
                | HostConnectionState::AuthFailed { .. }
                | HostConnectionState::KeyMismatch { .. }
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TailscalePeer {
    pub hostname: String,
    pub dns_name: String,
    pub ips: Vec<String>,
    pub os: String,
    pub online: bool,
}

impl TailscalePeer {
    pub fn preferred_ip(&self) -> Option<&str> {
        self.ips
            .iter()
            .find(|ip| ip.contains('.'))
            .or_else(|| self.ips.first())
            .map(String::as_str)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshHostEntry {
    pub alias: String,
    pub hostname: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl SshHostEntry {
    pub fn target_spec(&self) -> String {
        match &self.user {
            Some(u) if !u.is_empty() && !self.alias.contains('@') => {
                format!("{}@{}", u, self.alias)
            }
            _ => self.alias.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryCatalog {
    pub tailscale_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale_self_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale_self_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale_error: Option<String>,
    #[serde(default)]
    pub tailscale_peers: Vec<TailscalePeer>,
    #[serde(default)]
    pub ssh_hosts: Vec<SshHostEntry>,
}

/// Returns `true` for Tailscale CGNAT (`100.64.0.0/10`) and ULA (`fd7a:115c:a1e0::/48`) IPs.
pub fn is_tailscale_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 100 && (octets[1] & 0b1100_0000) == 64
        }
        std::net::IpAddr::V6(v6) => {
            let segs = v6.segments();
            segs[0] == 0xfd7a && segs[1] == 0x115c && segs[2] == 0xa1e0
        }
    }
}

/// Extracts the host/IP component from `host:port`, `[ipv6]:port`, or `canban://...` strings.
pub fn extract_daemon_host(addr: &str) -> &str {
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

/// Returns `true` when `addr` targets a `.ts.net` MagicDNS hostname or Tailscale IP address.
pub fn is_tailscale_addr(addr: &str) -> bool {
    let host = extract_daemon_host(addr);
    if host.eq_ignore_ascii_case("ts.net") || host.to_ascii_lowercase().ends_with(".ts.net") {
        return true;
    }
    host.parse::<std::net::IpAddr>().is_ok_and(is_tailscale_ip)
}

/// Format a copyable pairing URI (`canban://<addr>?pk=<server_pubkey>#<pairing_code>`).
pub fn format_pairing_uri(addr: &str, server_pubkey: &str, code: &str) -> String {
    let norm = normalize_daemon_addr(addr);
    let pk = server_pubkey.trim();
    if pk.is_empty() {
        format!("canban://{norm}#{}", code.trim())
    } else {
        format!("canban://{norm}?pk={pk}#{}", code.trim())
    }
}

/// Parse `canban://host:port?pk=<server_pubkey>#<pairing_code>`, `tcp://...`, or `host:port?pk=...#code`.
/// If `:port` is omitted, defaults to `DEFAULT_DAEMON_PORT`.
/// Non-Tailscale endpoints MUST include a valid `?pk=<server_pubkey>` query parameter.
pub fn parse_pairing_uri(raw: &str) -> Option<(String, Option<String>, String)> {
    let trimmed = raw.trim();
    let (before_frag, code_part) = trimmed.split_once('#')?;
    let code = code_part.trim();
    if !is_pairing_code(code) || code.len() < PAIRING_CODE_PREFIX.len() + 16 {
        return None;
    }
    let without_scheme = before_frag
        .strip_prefix("canban://")
        .or_else(|| before_frag.strip_prefix("tcp://"))
        .unwrap_or(before_frag);
    let (host_part, query_opt) = match without_scheme.split_once('?') {
        Some((h, q)) => (h.trim().trim_end_matches('/'), Some(q)),
        None => (without_scheme.trim().trim_end_matches('/'), None),
    };
    if host_part.is_empty() || host_part.eq_ignore_ascii_case("pair") {
        return None;
    }
    let mut server_pubkey = None;
    if let Some(query) = query_opt {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=')?;
            match k {
                "pk" => {
                    let pk = v.trim();
                    if !is_valid_noise_pubkey_b64(pk) {
                        return None;
                    }
                    server_pubkey = Some(pk.to_owned());
                }
                "v" | "a" => return None,
                _ => {}
            }
        }
    }
    let addr = normalize_daemon_addr(host_part);
    if addr.is_empty() {
        return None;
    }
    if !is_tailscale_addr(&addr) && server_pubkey.is_none() {
        return None;
    }
    Some((addr, server_pubkey, code.to_owned()))
}

/// Normalize `host` or `host:port` (including IPv6 `[::1]` / `[::1]:7680`) to `host:port`.
pub fn normalize_daemon_addr(raw: &str) -> String {
    let s = raw
        .trim()
        .strip_prefix("tcp://")
        .or_else(|| raw.trim().strip_prefix("canban://"))
        .unwrap_or(raw.trim())
        .trim_end_matches('/');
    if s.is_empty() {
        return String::new();
    }
    if s.starts_with('[') {
        if s.contains("]:") {
            return s.to_owned();
        }
        return format!("{s}:{DEFAULT_DAEMON_PORT}");
    }
    if let Some((_, port_str)) = s.rsplit_once(':')
        && !s[..s.len() - port_str.len() - 1].contains(':')
        && port_str.parse::<u16>().is_ok()
    {
        return s.to_owned();
    }
    if s.contains(':') {
        format!("[{s}]:{DEFAULT_DAEMON_PORT}")
    } else {
        format!("{s}:{DEFAULT_DAEMON_PORT}")
    }
}

pub const QUALIFIED_HOST_PREFIX: &str = "host::";

pub fn qualify_host_id(host_id: &str, raw_id: &str) -> String {
    if split_qualified_host_id(raw_id).is_some() {
        return raw_id.to_owned();
    }
    format!("{QUALIFIED_HOST_PREFIX}{host_id}::{raw_id}")
}

pub fn split_qualified_host_id(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(QUALIFIED_HOST_PREFIX)?.split_once("::")
}

pub fn unqualified_entity_id(id: &str) -> &str {
    split_qualified_host_id(id).map_or(id, |(_, raw)| raw)
}

fn is_id_json_key(key: &str) -> bool {
    key == "id"
        || key == "terminal"
        || key == "target_id"
        || key == "merging_cards"
        || key.ends_with("_id")
        || key.ends_with("_ids")
}

/// Unwraps `host::<host_id>::<raw_id>` identifiers inside RPC params before forwarding to `host_id`.
pub fn strip_qualified_host_ids(value: &mut serde_json::Value, host_id: &str) {
    match value {
        serde_json::Value::Array(arr) => {
            for item in arr {
                strip_qualified_host_ids(item, host_id);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if is_id_json_key(k) {
                    strip_id_value(v, host_id);
                } else if matches!(
                    v,
                    serde_json::Value::Object(_) | serde_json::Value::Array(_)
                ) {
                    strip_qualified_host_ids(v, host_id);
                }
            }
        }
        _ => {}
    }
}

fn strip_id_value(value: &mut serde_json::Value, host_id: &str) {
    match value {
        serde_json::Value::String(s) => {
            if let Some((h, raw)) = split_qualified_host_id(s)
                && h == host_id
            {
                *s = raw.to_owned();
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                strip_id_value(item, host_id);
            }
        }
        serde_json::Value::Object(_) => strip_qualified_host_ids(value, host_id),
        _ => {}
    }
}

/// Re-qualifies ID fields in a remote host's JSON response whenever the entity is known to be
/// qualified in `entity_hosts` or represents a remote `__global` schedule/workflow identifier.
pub fn qualify_remote_value_ids(
    value: &mut serde_json::Value,
    host_id: &str,
    entity_hosts: &BTreeMap<String, String>,
) {
    match value {
        serde_json::Value::Array(arr) => {
            for item in arr {
                qualify_remote_value_ids(item, host_id, entity_hosts);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if is_id_json_key(k) {
                    qualify_id_value(v, host_id, entity_hosts);
                } else if matches!(
                    v,
                    serde_json::Value::Object(_) | serde_json::Value::Array(_)
                ) {
                    qualify_remote_value_ids(v, host_id, entity_hosts);
                }
            }
        }
        _ => {}
    }
}

fn is_remote_global_qualified_target(raw: &str) -> bool {
    raw.starts_with("schedule:__global:")
        || raw.starts_with("scheduled:__global:")
        || raw.starts_with("__global:")
}

fn qualify_id_value(
    value: &mut serde_json::Value,
    host_id: &str,
    entity_hosts: &BTreeMap<String, String>,
) {
    match value {
        serde_json::Value::String(s) => {
            if s.is_empty() || split_qualified_host_id(s).is_some() {
                return;
            }
            let q = qualify_host_id(host_id, s);
            if entity_hosts.get(&q).map(String::as_str) == Some(host_id)
                || is_remote_global_qualified_target(s)
            {
                *s = q;
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                qualify_id_value(item, host_id, entity_hosts);
            }
        }
        serde_json::Value::Object(_) => qualify_remote_value_ids(value, host_id, entity_hosts),
        _ => {}
    }
}

fn remap_string(id: &mut String, map: &std::collections::HashMap<String, String>) {
    if let Some(mapped) = map.get(id.as_str()) {
        *id = mapped.clone();
    }
}

fn remap_opt(id: &mut Option<String>, map: &std::collections::HashMap<String, String>) {
    if let Some(cur) = id.as_deref()
        && let Some(mapped) = map.get(cur)
    {
        *id = Some(mapped.clone());
    }
}

fn resolve_runtime_id(
    entity_hosts: &BTreeMap<String, String>,
    host_id: &str,
    raw_id: &str,
    term_map: &std::collections::HashMap<String, String>,
) -> String {
    if let Some(mapped) = term_map.get(raw_id) {
        return mapped.clone();
    }
    let qualified = qualify_host_id(host_id, raw_id);
    if entity_hosts.get(&qualified).map(String::as_str) == Some(host_id)
        || entity_hosts
            .get(raw_id)
            .is_some_and(|owner| owner != host_id)
    {
        qualified
    } else {
        raw_id.to_owned()
    }
}

impl Snapshot {
    pub fn register_host_entities(&mut self, host_id: &str) {
        let h = host_id.to_owned();
        for s in &mut self.workflow_sources {
            if s.host_id.is_empty() {
                s.host_id = h.clone();
            }
        }
        for r in &mut self.workflow_revisions {
            if r.host_id.is_empty() {
                r.host_id = h.clone();
            }
        }
        for st in &mut self.schedule_states {
            if st.host_id.is_empty() {
                st.host_id = h.clone();
            }
        }
        for sr in &mut self.scheduled_runs {
            if sr.host_id.is_empty() {
                sr.host_id = h.clone();
            }
        }
        for p in &self.projects {
            self.entity_hosts.insert(p.id.clone(), h.clone());
        }
        for c in &self.cards {
            self.entity_hosts.insert(c.id.clone(), h.clone());
        }
        for t in &self.terminals {
            self.entity_hosts.insert(t.id.clone(), h.clone());
        }
        for w in &self.worktrees {
            self.entity_hosts.insert(w.id.clone(), h.clone());
        }
        for r in &self.workflow_runs {
            self.entity_hosts.insert(r.id.clone(), h.clone());
        }
        for s in &self.workflow_sources {
            self.entity_hosts.insert(s.id.clone(), h.clone());
        }
        for r in &self.workflow_revisions {
            self.entity_hosts.insert(r.id.clone(), h.clone());
        }
        for cs in &self.configuration_sessions {
            self.entity_hosts.insert(cs.id.clone(), h.clone());
        }
        for mb in &self.merge_batches {
            self.entity_hosts.insert(mb.id.clone(), h.clone());
        }
        for st in &self.schedule_states {
            self.entity_hosts.insert(st.id.clone(), h.clone());
        }
        for sr in &self.scheduled_runs {
            self.entity_hosts.insert(sr.id.clone(), h.clone());
        }
        for gi in &self.github_issues {
            self.entity_hosts.insert(gi.id.clone(), h.clone());
        }
        for cr in &self.card_reads {
            self.entity_hosts.insert(cr.id.clone(), h.clone());
        }
        for rem in &self.reminders {
            self.entity_hosts.insert(rem.id.clone(), h.clone());
        }
        for rep in &self.reports {
            self.entity_hosts.insert(rep.id.clone(), h.clone());
        }
        for note in &self.notes {
            self.entity_hosts.insert(note.id.clone(), h.clone());
        }
        for notice in &self.notices {
            self.entity_hosts.insert(notice.id.clone(), h.clone());
        }
        for art in &self.artifacts {
            self.entity_hosts.insert(art.id.clone(), h.clone());
        }
        for conv in &self.conversations {
            self.entity_hosts.insert(conv.id.clone(), h.clone());
        }
        for q in &self.questions {
            self.entity_hosts.insert(q.id.clone(), h.clone());
        }
        for ne in &self.note_events {
            self.entity_hosts.insert(ne.id.clone(), h.clone());
        }
        for rd in &self.review_drafts {
            self.entity_hosts.insert(rd.card_id.clone(), h.clone());
        }
        for sub in &self.event_subscriptions {
            self.entity_hosts.insert(sub.id.clone(), h.clone());
        }
    }

    pub fn merge_remote_snapshot(&mut self, mut remote: Snapshot, summary: HostSummary) {
        use std::collections::{HashMap, HashSet};

        let host_id = summary.id.clone();
        remote.register_host_entities(&host_id);
        self.host_summaries.insert(host_id.clone(), summary);

        let existing_projects: HashSet<String> =
            self.projects.iter().map(|p| p.id.clone()).collect();
        let existing_cards: HashSet<String> = self.cards.iter().map(|c| c.id.clone()).collect();
        let existing_terminals: HashSet<String> =
            self.terminals.iter().map(|t| t.id.clone()).collect();
        let existing_worktrees: HashSet<String> =
            self.worktrees.iter().map(|w| w.id.clone()).collect();
        let existing_workflow_sources: HashSet<String> =
            self.workflow_sources.iter().map(|w| w.id.clone()).collect();
        let existing_workflow_revisions: HashSet<String> = self
            .workflow_revisions
            .iter()
            .map(|r| r.id.clone())
            .collect();
        let existing_workflow_runs: HashSet<String> =
            self.workflow_runs.iter().map(|r| r.id.clone()).collect();
        let existing_config_sessions: HashSet<String> = self
            .configuration_sessions
            .iter()
            .map(|c| c.id.clone())
            .collect();
        let existing_merge_batches: HashSet<String> =
            self.merge_batches.iter().map(|b| b.id.clone()).collect();
        let existing_schedule_states: HashSet<String> =
            self.schedule_states.iter().map(|s| s.id.clone()).collect();
        let existing_scheduled_runs: HashSet<String> =
            self.scheduled_runs.iter().map(|r| r.id.clone()).collect();
        let existing_github_issues: HashSet<String> =
            self.github_issues.iter().map(|g| g.id.clone()).collect();
        let existing_card_reads: HashSet<String> =
            self.card_reads.iter().map(|c| c.id.clone()).collect();
        let existing_reminders: HashSet<String> =
            self.reminders.iter().map(|r| r.id.clone()).collect();
        let existing_reports: HashSet<String> = self.reports.iter().map(|r| r.id.clone()).collect();
        let existing_notes: HashSet<String> = self.notes.iter().map(|n| n.id.clone()).collect();
        let existing_notices: HashSet<String> = self.notices.iter().map(|n| n.id.clone()).collect();
        let existing_artifacts: HashSet<String> =
            self.artifacts.iter().map(|a| a.id.clone()).collect();
        let existing_conversations: HashSet<String> =
            self.conversations.iter().map(|c| c.id.clone()).collect();
        let existing_questions: HashSet<String> =
            self.questions.iter().map(|q| q.id.clone()).collect();
        let existing_note_events: HashSet<String> =
            self.note_events.iter().map(|e| e.id.clone()).collect();
        let existing_review_drafts: HashSet<String> = self
            .review_drafts
            .iter()
            .map(|d| d.card_id.clone())
            .collect();
        let existing_event_subscriptions: HashSet<String> = self
            .event_subscriptions
            .iter()
            .map(|s| s.id.clone())
            .collect();

        let mut project_map: HashMap<String, String> = HashMap::new();
        let mut card_map: HashMap<String, String> = HashMap::new();
        let mut terminal_map: HashMap<String, String> = HashMap::new();
        let mut config_session_map: HashMap<String, String> = HashMap::new();
        let mut merge_batch_map: HashMap<String, String> = HashMap::new();
        let mut worktree_map: HashMap<String, String> = HashMap::new();
        let mut workflow_source_map: HashMap<String, String> = HashMap::new();
        let mut workflow_revision_map: HashMap<String, String> = HashMap::new();
        let mut workflow_run_map: HashMap<String, String> = HashMap::new();
        let mut schedule_map: HashMap<String, String> = HashMap::new();
        let mut scheduled_run_map: HashMap<String, String> = HashMap::new();
        let mut note_map: HashMap<String, String> = HashMap::new();

        // 1. Projects
        for mut p in remote.projects {
            if existing_projects.contains(&p.id) {
                let q = qualify_host_id(&host_id, &p.id);
                project_map.insert(p.id, q.clone());
                p.id = q;
            }
            self.entity_hosts
                .entry(p.id.clone())
                .or_insert_with(|| host_id.clone());
            if !existing_projects.contains(&p.id) {
                self.projects.push(p);
            }
        }

        // 2. Cards (build card_map before runs/terminals/comments/drafts)
        for mut c in remote.cards {
            remap_string(&mut c.project_id, &project_map);
            if existing_cards.contains(&c.id) {
                let q = qualify_host_id(&host_id, &c.id);
                card_map.insert(c.id, q.clone());
                c.id = q;
            }
            self.cards.push(c);
        }

        // 3. Worktrees
        for mut w in remote.worktrees {
            remap_string(&mut w.project_id, &project_map);
            if existing_worktrees.contains(&w.id) {
                let q = qualify_host_id(&host_id, &w.id);
                worktree_map.insert(w.id, q.clone());
                w.id = q;
            }
            self.entity_hosts
                .entry(w.id.clone())
                .or_insert_with(|| host_id.clone());
            self.worktrees.push(w);
        }

        // 4. Workflow sources & revisions (always qualify remote __global entries or collisions)
        for mut wr in remote.workflow_revisions {
            if wr.host_id.is_empty() {
                wr.host_id = host_id.clone();
            }
            let is_global = wr.project_id == GLOBAL_PROJECT_ID;
            remap_string(&mut wr.project_id, &project_map);
            if is_global || existing_workflow_revisions.contains(&wr.id) {
                let q = qualify_host_id(&host_id, &wr.id);
                workflow_revision_map.insert(wr.id, q.clone());
                wr.id = q;
            }
            self.entity_hosts
                .entry(wr.id.clone())
                .or_insert_with(|| host_id.clone());
            self.workflow_revisions.push(wr);
        }
        for mut ws in remote.workflow_sources {
            if ws.host_id.is_empty() {
                ws.host_id = host_id.clone();
            }
            let is_global = ws.project_id == GLOBAL_PROJECT_ID;
            remap_string(&mut ws.project_id, &project_map);
            remap_string(&mut ws.revision_id, &workflow_revision_map);
            if is_global || existing_workflow_sources.contains(&ws.id) {
                let q = qualify_host_id(&host_id, &ws.id);
                workflow_source_map.insert(ws.id, q.clone());
                ws.id = q;
            }
            self.entity_hosts
                .entry(ws.id.clone())
                .or_insert_with(|| host_id.clone());
            self.workflow_sources.push(ws);
        }

        // 5. Terminals (and AgentRuns) & ScheduledRuns & ScheduleStates
        // Pre-pass shared-ID entities (`scheduled_runs`, `configuration_sessions`, `merge_batches`,
        // and `terminals`) together so cross-references and `id == terminal.id` joins stay aligned.
        for sr in &remote.scheduled_runs {
            let is_global = sr.project_id == GLOBAL_PROJECT_ID;
            if is_global
                || existing_scheduled_runs.contains(&sr.id)
                || existing_terminals.contains(&sr.id)
            {
                let q = qualify_host_id(&host_id, &sr.id);
                scheduled_run_map.insert(sr.id.clone(), q.clone());
                terminal_map.insert(sr.id.clone(), q);
            }
        }
        for cs in &remote.configuration_sessions {
            let is_global = cs.project_id == GLOBAL_PROJECT_ID;
            if is_global
                || existing_config_sessions.contains(&cs.id)
                || existing_terminals.contains(&cs.id)
            {
                let q = qualify_host_id(&host_id, &cs.id);
                config_session_map.insert(cs.id.clone(), q.clone());
                terminal_map.insert(cs.id.clone(), q);
            }
        }
        for mb in &remote.merge_batches {
            if existing_merge_batches.contains(&mb.id) || existing_terminals.contains(&mb.id) {
                let q = qualify_host_id(&host_id, &mb.id);
                merge_batch_map.insert(mb.id.clone(), q.clone());
                terminal_map.insert(mb.id.clone(), q);
            }
        }
        for ss in &remote.schedule_states {
            let is_global = ss.project_id == GLOBAL_PROJECT_ID;
            if is_global || existing_schedule_states.contains(&ss.id) {
                schedule_map.insert(ss.id.clone(), qualify_host_id(&host_id, &ss.id));
            }
        }
        for r in &remote.workflow_runs {
            if existing_workflow_runs.contains(&r.id) {
                workflow_run_map.insert(r.id.clone(), qualify_host_id(&host_id, &r.id));
            }
        }
        for t in &remote.terminals {
            if existing_terminals.contains(&t.id) {
                terminal_map
                    .entry(t.id.clone())
                    .or_insert_with(|| qualify_host_id(&host_id, &t.id));
            }
        }

        // Fixup Card schedule_source and human_gate refs now that maps are populated.
        for mut c_fix in std::mem::take(&mut self.cards) {
            if !existing_cards.contains(&c_fix.id) || card_map.values().any(|v| v == &c_fix.id) {
                // Card either wasn't in `self` originally or was remapped for this remote host.
                if let Some(ref mut ss_ref) = c_fix.schedule_source {
                    remap_string(&mut ss_ref.schedule_id, &schedule_map);
                    remap_string(&mut ss_ref.run_id, &scheduled_run_map);
                }
                if let Some(ref mut gate) = c_fix.human_gate {
                    remap_opt(&mut gate.terminal_id, &terminal_map);
                }
            }
            self.cards.push(c_fix);
        }

        for c in &self.cards {
            if !existing_cards.contains(&c.id) || card_map.values().any(|v| v == &c.id) {
                self.entity_hosts
                    .entry(c.id.clone())
                    .or_insert_with(|| host_id.clone());
            }
        }

        for mut r in remote.workflow_runs {
            remap_string(&mut r.project_id, &project_map);
            remap_opt(&mut r.card_id, &card_map);
            remap_opt(&mut r.terminal_id, &terminal_map);
            remap_string(&mut r.id, &workflow_run_map);
            self.entity_hosts
                .entry(r.id.clone())
                .or_insert_with(|| host_id.clone());
            self.workflow_runs.push(r);
        }

        for mut ss in remote.schedule_states {
            if ss.host_id.is_empty() {
                ss.host_id = host_id.clone();
            }
            remap_string(&mut ss.project_id, &project_map);
            remap_string(&mut ss.id, &schedule_map);
            remap_opt(&mut ss.last_run_id, &scheduled_run_map);
            self.entity_hosts
                .entry(ss.id.clone())
                .or_insert_with(|| host_id.clone());
            self.schedule_states.push(ss);
        }

        for mut sr in remote.scheduled_runs {
            if sr.host_id.is_empty() {
                sr.host_id = host_id.clone();
            }
            remap_string(&mut sr.project_id, &project_map);
            remap_string(&mut sr.schedule_id, &schedule_map);
            remap_opt(&mut sr.card_id, &card_map);
            remap_opt(&mut sr.workflow_run_id, &workflow_run_map);
            remap_opt(&mut sr.terminal_id, &terminal_map);
            remap_string(&mut sr.id, &scheduled_run_map);
            self.entity_hosts
                .entry(sr.id.clone())
                .or_insert_with(|| host_id.clone());
            self.scheduled_runs.push(sr);
        }

        for mut t in remote.terminals {
            remap_string(&mut t.id, &terminal_map);
            remap_opt(&mut t.origin_card_id, &card_map);
            if let Some(ref mut a) = t.agent {
                remap_string(&mut a.project_id, &project_map);
                remap_string(&mut a.card_id, &card_map);
                remap_opt(&mut a.workflow_run_id, &workflow_run_map);
                remap_opt(&mut a.terminal, &terminal_map);
            }
            self.entity_hosts
                .entry(t.id.clone())
                .or_insert_with(|| host_id.clone());
            self.terminals.push(t);
        }

        for mut ctx in remote.terminal_contexts {
            remap_string(&mut ctx.terminal_id, &terminal_map);
            remap_opt(&mut ctx.project_id, &project_map);
            remap_opt(&mut ctx.card_id, &card_map);
            remap_opt(&mut ctx.worktree_id, &worktree_map);
            self.terminal_contexts.push(ctx);
        }

        for mut mc in remote.merging_cards {
            remap_string(&mut mc, &card_map);
            if !self.merging_cards.contains(&mc) {
                self.merging_cards.push(mc);
            }
        }

        for mut cs in remote.configuration_sessions {
            remap_string(&mut cs.project_id, &project_map);
            remap_string(&mut cs.id, &config_session_map);
            self.entity_hosts
                .entry(cs.id.clone())
                .or_insert_with(|| host_id.clone());
            self.configuration_sessions.push(cs);
        }

        for mut mb in remote.merge_batches {
            remap_string(&mut mb.project_id, &project_map);
            for item in &mut mb.items {
                remap_string(&mut item.card_id, &card_map);
                remap_string(&mut item.workflow_run_id, &workflow_run_map);
            }
            remap_string(&mut mb.id, &merge_batch_map);
            self.entity_hosts
                .entry(mb.id.clone())
                .or_insert_with(|| host_id.clone());
            self.merge_batches.push(mb);
        }

        for mut cr in remote.card_reads {
            remap_string(&mut cr.project_id, &project_map);
            remap_string(&mut cr.card_id, &card_map);
            if existing_card_reads.contains(&cr.id) {
                cr.id = qualify_host_id(&host_id, &cr.id);
            }
            self.entity_hosts
                .entry(cr.id.clone())
                .or_insert_with(|| host_id.clone());
            self.card_reads.push(cr);
        }

        for mut gi in remote.github_issues {
            remap_string(&mut gi.project_id, &project_map);
            if existing_github_issues.contains(&gi.id) {
                gi.id = qualify_host_id(&host_id, &gi.id);
            }
            self.entity_hosts
                .entry(gi.id.clone())
                .or_insert_with(|| host_id.clone());
            self.github_issues.push(gi);
        }

        for (mut cid, summary) in remote.card_summaries {
            remap_string(&mut cid, &card_map);
            self.card_summaries.entry(cid).or_insert(summary);
        }

        for mut rem in remote.reminders {
            remap_string(&mut rem.project_id, &project_map);
            remap_string(&mut rem.card_id, &card_map);
            if existing_reminders.contains(&rem.id) {
                rem.id = qualify_host_id(&host_id, &rem.id);
            }
            self.entity_hosts
                .entry(rem.id.clone())
                .or_insert_with(|| host_id.clone());
            self.reminders.push(rem);
        }

        for sid in remote.sound_event_ids {
            if !self.sound_event_ids.contains(&sid) {
                self.sound_event_ids.push(sid);
            }
        }

        for mut rep in remote.reports {
            remap_string(&mut rep.project_id, &project_map);
            remap_string(&mut rep.card_id, &card_map);
            remap_opt(&mut rep.terminal_id, &terminal_map);
            remap_opt(&mut rep.workflow_run_id, &workflow_run_map);
            if existing_reports.contains(&rep.id) {
                rep.id = qualify_host_id(&host_id, &rep.id);
            }
            self.entity_hosts
                .entry(rep.id.clone())
                .or_insert_with(|| host_id.clone());
            self.reports.push(rep);
        }

        for mut note in remote.notes {
            remap_string(&mut note.project_id, &project_map);
            for cid in &mut note.card_ids {
                remap_string(cid, &card_map);
            }
            if existing_notes.contains(&note.id) {
                let q = qualify_host_id(&host_id, &note.id);
                note_map.insert(note.id, q.clone());
                note.id = q;
            }
            self.entity_hosts
                .entry(note.id.clone())
                .or_insert_with(|| host_id.clone());
            self.notes.push(note);
        }

        for mut notice in remote.notices {
            remap_string(&mut notice.project_id, &project_map);
            remap_string(&mut notice.card_id, &card_map);
            if existing_notices.contains(&notice.id) {
                notice.id = qualify_host_id(&host_id, &notice.id);
            }
            self.entity_hosts
                .entry(notice.id.clone())
                .or_insert_with(|| host_id.clone());
            self.notices.push(notice);
        }

        for mut art in remote.artifacts {
            remap_string(&mut art.project_id, &project_map);
            remap_string(&mut art.card_id, &card_map);
            if existing_artifacts.contains(&art.id) {
                art.id = qualify_host_id(&host_id, &art.id);
            }
            self.entity_hosts
                .entry(art.id.clone())
                .or_insert_with(|| host_id.clone());
            self.artifacts.push(art);
        }

        for mut conv in remote.conversations {
            remap_string(&mut conv.project_id, &project_map);
            remap_string(&mut conv.card_id, &card_map);
            if existing_conversations.contains(&conv.id) {
                conv.id = qualify_host_id(&host_id, &conv.id);
            }
            self.entity_hosts
                .entry(conv.id.clone())
                .or_insert_with(|| host_id.clone());
            self.conversations.push(conv);
        }

        for mut q in remote.questions {
            remap_string(&mut q.project_id, &project_map);
            remap_string(&mut q.card_id, &card_map);
            remap_opt(&mut q.terminal_id, &terminal_map);
            if existing_questions.contains(&q.id) {
                q.id = qualify_host_id(&host_id, &q.id);
            }
            self.entity_hosts
                .entry(q.id.clone())
                .or_insert_with(|| host_id.clone());
            self.questions.push(q);
        }

        for mut ne in remote.note_events {
            remap_string(&mut ne.project_id, &project_map);
            remap_string(&mut ne.note_id, &note_map);
            remap_opt(&mut ne.card_id, &card_map);
            if existing_note_events.contains(&ne.id) {
                ne.id = qualify_host_id(&host_id, &ne.id);
            }
            self.entity_hosts
                .entry(ne.id.clone())
                .or_insert_with(|| host_id.clone());
            self.note_events.push(ne);
        }

        for mut rd in remote.review_drafts {
            remap_string(&mut rd.card_id, &card_map);
            if existing_review_drafts.contains(&rd.card_id) {
                continue;
            }
            self.entity_hosts
                .entry(rd.card_id.clone())
                .or_insert_with(|| host_id.clone());
            self.review_drafts.push(rd);
        }

        for mut status in remote.reclaim_status {
            remap_string(&mut status.terminal_id, &terminal_map);
            if !self
                .reclaim_status
                .iter()
                .any(|existing| existing.terminal_id == status.terminal_id)
            {
                self.reclaim_status.push(status);
            }
        }

        for (mut cid, mark) in remote.pending_sync {
            remap_string(&mut cid, &card_map);
            self.pending_sync.entry(cid).or_insert(mark);
        }

        for (mut k, v) in remote.github_sync {
            remap_string(&mut k, &project_map);
            self.github_sync.entry(k).or_insert(v);
        }

        for (key, entries) in remote.scoped_env {
            if let Some(pid) = key.strip_prefix("project:") {
                let target_pid = project_map.get(pid).map(String::as_str).unwrap_or(pid);
                self.scoped_env
                    .entry(Self::project_env_key(target_pid))
                    .or_insert(entries);
            } else if let Some(pid) = key.strip_prefix("agent:") {
                self.scoped_env
                    .entry(Self::agent_env_key(&host_id, pid))
                    .or_insert(entries);
            }
        }

        for mut sub in remote.event_subscriptions {
            remap_opt(&mut sub.selector.project_id, &project_map);
            remap_opt(&mut sub.selector.card_id, &card_map);
            if let SubscriptionTarget::Workflow { card_id, .. } = &mut sub.target {
                remap_opt(card_id, &card_map);
            }
            if existing_event_subscriptions.contains(&sub.id) {
                sub.id = qualify_host_id(&host_id, &sub.id);
            }
            self.entity_hosts
                .entry(sub.id.clone())
                .or_insert_with(|| host_id.clone());
            self.event_subscriptions.push(sub);
        }
    }

    pub fn host_of<'a>(&'a self, entity_id: &str) -> &'a str {
        if let Some((h, _)) = split_qualified_host_id(entity_id) {
            if h == "local" {
                return "local";
            }
            if let Some((k, _)) = self.host_summaries.get_key_value(h) {
                return k.as_str();
            }
        }
        self.entity_hosts
            .get(entity_id)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("local")
    }

    pub fn project_host<'a>(&'a self, project_id: &str) -> &'a str {
        self.host_of(project_id)
    }

    pub fn schedule_host<'a>(&'a self, schedule_id: &str) -> &'a str {
        if let Some(st) = self
            .schedule_states
            .iter()
            .find(|s| s.id == schedule_id && !s.host_id.is_empty())
        {
            return st.host_id.as_str();
        }
        self.host_of(schedule_id)
    }

    pub fn workflow_source_host<'a>(&'a self, source_id: &str) -> &'a str {
        if let Some(ws) = self
            .workflow_sources
            .iter()
            .find(|s| s.id == source_id && !s.host_id.is_empty())
        {
            return ws.host_id.as_str();
        }
        self.host_of(source_id)
    }

    pub fn has_duplicate_project_name(&self, project_id: &str) -> bool {
        let Some(target) = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name.trim())
        else {
            return false;
        };
        self.projects
            .iter()
            .filter(|p| p.name.trim() == target)
            .count()
            > 1
    }

    pub fn project_host_badge(&self, project_id: &str) -> Option<String> {
        let hid = self.project_host(project_id);
        if self.has_duplicate_project_name(project_id)
            || (hid != "local" && self.has_remote_hosts())
        {
            Some(self.host_display_name(hid).to_owned())
        } else {
            None
        }
    }

    pub fn project_display_name(&self, project_id: &str) -> String {
        let Some(p) = self.projects.iter().find(|p| p.id == project_id) else {
            return project_id.to_owned();
        };
        if let Some(badge) = self.project_host_badge(project_id) {
            format!("{} · {badge}", p.name)
        } else {
            p.name.clone()
        }
    }

    pub fn card_host<'a>(&'a self, card_id: &str) -> &'a str {
        if let Some(h) = self.entity_hosts.get(card_id).filter(|s| !s.is_empty()) {
            return h.as_str();
        }
        if let Some((h, _)) = split_qualified_host_id(card_id) {
            if h == "local" {
                return "local";
            }
            if let Some((k, _)) = self.host_summaries.get_key_value(h) {
                return k.as_str();
            }
        }
        if let Some(card) = self.cards.iter().find(|c| c.id == card_id) {
            return self.project_host(&card.project_id);
        }
        "local"
    }

    pub fn terminal_host<'a>(&'a self, terminal_id: &str) -> &'a str {
        if let Some(h) = self.entity_hosts.get(terminal_id).filter(|s| !s.is_empty()) {
            return h.as_str();
        }
        if let Some((h, _)) = split_qualified_host_id(terminal_id) {
            if h == "local" {
                return "local";
            }
            if let Some((k, _)) = self.host_summaries.get_key_value(h) {
                return k.as_str();
            }
        }
        if let Some(t) = self.terminals.iter().find(|t| t.id == terminal_id)
            && let Some(pid) = self.terminal_project_id(t)
        {
            return self.project_host(pid);
        }
        "local"
    }

    pub fn host_summary(&self, host_id: &str) -> Option<&HostSummary> {
        self.host_summaries.get(host_id)
    }

    pub fn host_display_name<'a>(&'a self, host_id: &'a str) -> &'a str {
        if host_id.is_empty() || host_id == "local" {
            return "This Mac";
        }
        self.host_summaries
            .get(host_id)
            .map(|h| h.name.as_str())
            .unwrap_or(host_id)
    }

    pub fn is_host_offline(&self, host_id: &str) -> bool {
        if host_id.is_empty() || host_id == "local" {
            return false;
        }
        self.host_summaries
            .get(host_id)
            .is_some_and(HostSummary::is_offline)
    }

    pub fn has_remote_hosts(&self) -> bool {
        self.host_summaries.keys().any(|k| k != "local")
    }

    /// Apply a bounded runtime state refresh while preserving durable workspace state
    /// and valid `started_at` timestamps backfilled during the full `app.state` fetch.
    pub fn apply_runtime_state(&mut self, mut runtime: RuntimeState) {
        for next in &mut runtime.terminals {
            let Some(next_agent) = &mut next.agent else {
                continue;
            };
            if next_agent
                .started_at
                .is_some_and(|ts| ts > 1_000_000_000_000)
            {
                continue;
            }
            if let Some(prev_started) = self
                .terminals
                .iter()
                .find(|prev| prev.id == next.id)
                .and_then(|prev| prev.agent.as_ref())
                .filter(|prev_agent| prev_agent.number == next_agent.number)
                .and_then(|prev_agent| prev_agent.started_at)
                .filter(|&ts| ts > 1_000_000_000_000)
            {
                next_agent.started_at = Some(prev_started);
            }
        }

        let prev_terminal_ids: Vec<String> = self.terminals.iter().map(|t| t.id.clone()).collect();
        let next_terminal_ids: std::collections::HashSet<&str> =
            runtime.terminals.iter().map(|t| t.id.as_str()).collect();
        let retained_non_terminal_ids: std::collections::HashSet<&str> = self
            .projects
            .iter()
            .map(|p| p.id.as_str())
            .chain(self.cards.iter().map(|c| c.id.as_str()))
            .chain(self.worktrees.iter().map(|w| w.id.as_str()))
            .chain(self.workflow_runs.iter().map(|r| r.id.as_str()))
            .chain(self.workflow_sources.iter().map(|s| s.id.as_str()))
            .chain(self.workflow_revisions.iter().map(|r| r.id.as_str()))
            .chain(self.configuration_sessions.iter().map(|c| c.id.as_str()))
            .chain(self.merge_batches.iter().map(|b| b.id.as_str()))
            .chain(self.schedule_states.iter().map(|s| s.id.as_str()))
            .chain(self.scheduled_runs.iter().map(|r| r.id.as_str()))
            .collect();

        for old_id in prev_terminal_ids {
            if !next_terminal_ids.contains(old_id.as_str())
                && !retained_non_terminal_ids.contains(old_id.as_str())
            {
                self.entity_hosts.remove(&old_id);
            }
        }
        for (tid, host) in runtime.terminal_hosts {
            self.entity_hosts.entry(tid).or_insert(host);
        }
        for (host_id, active) in runtime.host_active_agents {
            if let Some(summary) = self.host_summaries.get_mut(&host_id) {
                summary.active_agents = active;
            }
        }

        self.terminals = runtime.terminals;
        self.terminal_contexts = runtime.terminal_contexts;
        self.reclaim_status = runtime.reclaim_status;
        self.merging_cards = runtime.merging_cards;
    }
}

impl RuntimeState {
    pub fn register_host(&mut self, host_id: &str) {
        let host = if host_id.is_empty() { "local" } else { host_id };
        for t in &self.terminals {
            self.terminal_hosts
                .entry(t.id.clone())
                .or_insert_with(|| host.to_owned());
        }
    }

    pub fn merge_remote(&mut self, remote: RuntimeState, host_id: &str) {
        let empty = BTreeMap::new();
        self.merge_remote_with_owners(remote, host_id, &empty);
    }

    pub fn merge_remote_for_snapshot(
        &mut self,
        remote: RuntimeState,
        host_id: &str,
        snapshot: Option<&Snapshot>,
    ) {
        if let Some(s) = snapshot {
            self.merge_remote_with_owners(remote, host_id, &s.entity_hosts);
        } else {
            self.merge_remote(remote, host_id);
        }
    }

    pub fn merge_remote_with_owners(
        &mut self,
        mut remote: RuntimeState,
        host_id: &str,
        entity_hosts: &BTreeMap<String, String>,
    ) {
        use std::collections::{HashMap, HashSet};

        remote.register_host(host_id);
        let existing_terminals: HashSet<String> =
            self.terminals.iter().map(|t| t.id.clone()).collect();
        let mut term_map: HashMap<String, String> = HashMap::new();

        for t in &remote.terminals {
            let q = qualify_host_id(host_id, &t.id);
            if existing_terminals.contains(&t.id)
                || entity_hosts.get(&q).map(String::as_str) == Some(host_id)
                || entity_hosts
                    .get(&t.id)
                    .is_some_and(|owner| owner != host_id)
            {
                term_map.insert(t.id.clone(), q);
            }
        }

        for (raw_tid, host) in remote.terminal_hosts {
            let tid = resolve_runtime_id(entity_hosts, host_id, &raw_tid, &term_map);
            self.terminal_hosts.entry(tid).or_insert(host);
        }
        self.host_active_agents.extend(remote.host_active_agents);

        for mut t in remote.terminals {
            t.id = resolve_runtime_id(entity_hosts, host_id, &t.id, &term_map);
            if let Some(ref mut cid) = t.origin_card_id {
                *cid = resolve_runtime_id(entity_hosts, host_id, cid, &term_map);
            }
            if let Some(ref mut a) = t.agent {
                a.project_id = resolve_runtime_id(entity_hosts, host_id, &a.project_id, &term_map);
                a.card_id = resolve_runtime_id(entity_hosts, host_id, &a.card_id, &term_map);
                if let Some(ref mut wrid) = a.workflow_run_id {
                    *wrid = resolve_runtime_id(entity_hosts, host_id, wrid, &term_map);
                }
                if let Some(ref mut term) = a.terminal {
                    *term = resolve_runtime_id(entity_hosts, host_id, term, &term_map);
                }
            }
            if !existing_terminals.contains(&t.id) {
                self.terminals.push(t);
            }
        }

        for mut ctx in remote.terminal_contexts {
            ctx.terminal_id =
                resolve_runtime_id(entity_hosts, host_id, &ctx.terminal_id, &term_map);
            if let Some(ref mut pid) = ctx.project_id {
                *pid = resolve_runtime_id(entity_hosts, host_id, pid, &term_map);
            }
            if let Some(ref mut cid) = ctx.card_id {
                *cid = resolve_runtime_id(entity_hosts, host_id, cid, &term_map);
            }
            if let Some(ref mut wid) = ctx.worktree_id {
                *wid = resolve_runtime_id(entity_hosts, host_id, wid, &term_map);
            }
            self.terminal_contexts.push(ctx);
        }

        for mut status in remote.reclaim_status {
            status.terminal_id =
                resolve_runtime_id(entity_hosts, host_id, &status.terminal_id, &term_map);
            if !self
                .reclaim_status
                .iter()
                .any(|existing| existing.terminal_id == status.terminal_id)
            {
                self.reclaim_status.push(status);
            }
        }

        for mc in remote.merging_cards {
            let mapped = resolve_runtime_id(entity_hosts, host_id, &mc, &term_map);
            if !self.merging_cards.contains(&mapped) {
                self.merging_cards.push(mapped);
            }
        }
    }
}

#[cfg(test)]
mod host_tests {
    use super::*;

    #[test]
    fn pairing_uri_round_trips_and_normalizes_default_ports() {
        let code = "pair_0123456789abcdef0123456789abcdef";
        let pk = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let uri = format_pairing_uri("100.84.12.5:7680", pk, code);
        assert_eq!(uri, format!("canban://100.84.12.5:7680?pk={pk}#{code}"));
        assert_eq!(
            parse_pairing_uri(&uri),
            Some(("100.84.12.5:7680".into(), Some(pk.into()), code.into()))
        );
        assert_eq!(
            parse_pairing_uri(&format!("100.84.12.5#{code}")),
            Some(("100.84.12.5:7680".into(), None, code.into()))
        );
        // Non-Tailscale URI without `?pk=` MUST be rejected.
        assert_eq!(
            parse_pairing_uri(&format!("canban://192.168.1.20:7680#{code}")),
            None
        );
        assert_eq!(
            parse_pairing_uri(&format!("canban://192.168.1.20:7680?pk={pk}#{code}")),
            Some(("192.168.1.20:7680".into(), Some(pk.into()), code.into()))
        );
        assert_eq!(parse_pairing_uri("canban://100.84.12.5#short"), None);
    }
}
