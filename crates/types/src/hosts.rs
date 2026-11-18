use super::*;

pub const DEFAULT_DAEMON_PORT: u16 = 7680;

fn default_daemon_port() -> u16 {
    DEFAULT_DAEMON_PORT
}

fn default_enabled() -> bool {
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
}

impl Default for ServerListenConfig {
    fn default() -> Self {
        Self {
            tailscale: false,
            port: DEFAULT_DAEMON_PORT,
            listen: None,
        }
    }
}

impl ServerListenConfig {
    pub fn tcp_enabled(&self) -> bool {
        self.tailscale || self.listen.as_deref().is_some_and(|s| !s.trim().is_empty())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostTransport {
    Local,
    Tcp {
        addr: String,
        token: String,
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
            HostConnectionState::ApiMismatch { .. } | HostConnectionState::AuthFailed { .. }
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

/// Format a copyable pairing URI (`canban://<addr>#<token>`).
pub fn format_pairing_uri(addr: &str, token: &str) -> String {
    format!("canban://{}#{}", addr.trim(), token.trim())
}

/// Parse `canban://host:port#token`, `tcp://host:port#token`, or `host:port#token`.
/// If `:port` is omitted, defaults to `DEFAULT_DAEMON_PORT`.
pub fn parse_pairing_uri(raw: &str) -> Option<(String, String)> {
    let trimmed = raw.trim();
    let without_scheme = trimmed
        .strip_prefix("canban://")
        .or_else(|| trimmed.strip_prefix("tcp://"))
        .unwrap_or(trimmed);
    let (host_part, token_part) = without_scheme.split_once('#')?;
    let host_part = host_part.trim().trim_end_matches('/');
    let token = token_part.trim();
    if host_part.is_empty() || token.len() < 16 {
        return None;
    }
    let addr = normalize_daemon_addr(host_part);
    Some((addr, token.to_owned()))
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

impl Snapshot {
    pub fn register_host_entities(&mut self, host_id: &str) {
        let h = host_id.to_owned();
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
    }

    pub fn merge_remote_snapshot(&mut self, mut remote: Snapshot, summary: HostSummary) {
        let host_id = summary.id.clone();
        remote.register_host_entities(&host_id);
        self.github_sync.extend(remote.github_sync.clone());
        for (k, v) in remote.entity_hosts {
            self.entity_hosts.insert(k, v);
        }
        self.host_summaries.insert(host_id, summary);

        let existing_projects: std::collections::HashSet<String> =
            self.projects.iter().map(|p| p.id.clone()).collect();
        for p in remote.projects {
            if !existing_projects.contains(&p.id) {
                self.projects.push(p);
            }
        }

        let existing_cards: std::collections::HashSet<String> =
            self.cards.iter().map(|c| c.id.clone()).collect();
        for c in remote.cards {
            if !existing_cards.contains(&c.id) {
                self.cards.push(c);
            }
        }

        let existing_terminals: std::collections::HashSet<String> =
            self.terminals.iter().map(|t| t.id.clone()).collect();
        for t in remote.terminals {
            if !existing_terminals.contains(&t.id) {
                self.terminals.push(t);
            }
        }

        self.terminal_contexts.extend(remote.terminal_contexts);
        for mc in remote.merging_cards {
            if !self.merging_cards.contains(&mc) {
                self.merging_cards.push(mc);
            }
        }
        self.worktrees.extend(remote.worktrees);
        self.workflow_runs.extend(remote.workflow_runs);
        self.workflow_sources.extend(remote.workflow_sources);
        self.workflow_revisions.extend(remote.workflow_revisions);
        self.event_subscriptions.extend(remote.event_subscriptions);
        self.configuration_sessions
            .extend(remote.configuration_sessions);
        self.merge_batches.extend(remote.merge_batches);
        self.schedule_states.extend(remote.schedule_states);
        self.scheduled_runs.extend(remote.scheduled_runs);
        self.card_reads.extend(remote.card_reads);
        self.github_issues.extend(remote.github_issues);
        self.events.extend(remote.events);
        self.reports.extend(remote.reports);
        self.notes.extend(remote.notes);
        self.notices.extend(remote.notices);
        self.artifacts.extend(remote.artifacts);
        self.conversations.extend(remote.conversations);
        self.questions.extend(remote.questions);
        self.note_events.extend(remote.note_events);
    }

    pub fn host_of<'a>(&'a self, entity_id: &str) -> &'a str {
        self.entity_hosts
            .get(entity_id)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("local")
    }

    pub fn project_host<'a>(&'a self, project_id: &str) -> &'a str {
        self.host_of(project_id)
    }

    pub fn card_host<'a>(&'a self, card_id: &str) -> &'a str {
        if let Some(h) = self.entity_hosts.get(card_id).filter(|s| !s.is_empty()) {
            return h.as_str();
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
}

#[cfg(test)]
mod host_tests {
    use super::*;

    #[test]
    fn pairing_uri_round_trips_and_normalizes_default_ports() {
        let token = "0123456789abcdef0123456789abcdef";
        let uri = format_pairing_uri("100.84.12.5:7680", token);
        assert_eq!(uri, format!("canban://100.84.12.5:7680#{token}"));
        assert_eq!(
            parse_pairing_uri(&uri),
            Some(("100.84.12.5:7680".into(), token.into()))
        );
        assert_eq!(
            parse_pairing_uri(&format!("100.84.12.5#{token}")),
            Some(("100.84.12.5:7680".into(), token.into()))
        );
        assert_eq!(parse_pairing_uri("canban://100.84.12.5#short"), None);
    }

    #[test]
    fn merge_remote_snapshot_indexes_ownership_and_preserves_local_github() {
        let mut local = Snapshot::default();
        local.github.login = Some("local-user".into());
        local.register_host_entities("local");

        let mut remote = Snapshot::default();
        remote.github.login = Some("remote-other".into());
        remote.projects.push(Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: vec![],
            id: "proj-rem".into(),
            name: "Linux Core".into(),
            directory: "/home/dev/core".into(),
            location: None,
            workflow: Workflow::default(),
            target_branch: Some("main".into()),
            created_at: 1,
        });

        let summary = HostSummary {
            id: "home-linux".into(),
            name: "home-linux".into(),
            is_local: false,
            transport_label: "Tailscale / TCP".into(),
            endpoint_label: "100.84.12.5:7680".into(),
            ssh_target: Some("jjy@100.84.12.5".into()),
            server_id: Some("srv-remote".into()),
            connection: HostConnectionState::Offline {
                error: "Network unreachable".into(),
                cached_at: Some(12345),
            },
            active_agents: 2,
            max_agents: 8,
            project_count: 1,
            reconnect_count: 1,
            last_synced_at: Some(12345),
        };
        local.merge_remote_snapshot(remote, summary);

        assert_eq!(local.github.login.as_deref(), Some("local-user"));
        assert_eq!(local.project_host("proj-rem"), "home-linux");
        assert_eq!(local.host_display_name("home-linux"), "home-linux");
        assert_eq!(local.host_display_name("local"), "This Mac");
        assert!(local.is_host_offline("home-linux"));
        assert!(!local.is_host_offline("local"));
    }

    #[test]
    fn updating_connection_state_serializes_and_formats_status_cleanly() {
        let generic = HostConnectionState::Updating {
            version: String::new(),
        };
        assert!(!generic.is_connected());
        assert!(generic.is_updating());
        assert_eq!(generic.short_status(), "Updating daemon…");
        assert_eq!(generic.error_detail(), None);

        let versioned = HostConnectionState::Updating {
            version: "20261116-1".into(),
        };
        assert_eq!(versioned.short_status(), "Updating to 20261116-1…");

        let json = serde_json::to_value(&versioned).unwrap();
        assert_eq!(json["state"], "updating");
        assert_eq!(json["version"], "20261116-1");
        let roundtrip: HostConnectionState = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip, versioned);

        let summary = HostSummary {
            id: "home-linux".into(),
            name: "home-linux".into(),
            is_local: false,
            transport_label: "Tailscale / TCP".into(),
            endpoint_label: "100.84.12.5:7680".into(),
            ssh_target: None,
            server_id: None,
            connection: generic,
            active_agents: 0,
            max_agents: 8,
            project_count: 1,
            reconnect_count: 0,
            last_synced_at: None,
        };
        assert!(summary.is_updating());
        assert!(summary.is_offline());
        assert!(!summary.needs_attention());
    }
}
