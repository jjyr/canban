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
    #[serde(default)]
    pub allow_insecure_bind: bool,
}

impl Default for ServerListenConfig {
    fn default() -> Self {
        Self {
            tailscale: false,
            port: DEFAULT_DAEMON_PORT,
            listen: None,
            allow_insecure_bind: false,
        }
    }
}

pub const PAIRING_CODE_PREFIX: &str = "pair_";
pub const DEVICE_TOKEN_PREFIX: &str = "dev_";

pub fn is_pairing_code(token: &str) -> bool {
    token.trim().starts_with(PAIRING_CODE_PREFIX)
}

pub fn is_device_token(token: &str) -> bool {
    token.trim().starts_with(DEVICE_TOKEN_PREFIX)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<u64>,
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
        for (key, entries) in remote.scoped_env {
            if key.starts_with("project:") {
                self.scoped_env.insert(key, entries);
            } else if let Some(pid) = key.strip_prefix("agent:") {
                self.scoped_env
                    .insert(Self::agent_env_key(&host_id, pid), entries);
            }
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
        self.card_summaries.extend(remote.card_summaries);
        self.reminders.extend(remote.reminders);
        self.sound_event_ids.extend(remote.sound_event_ids);
        self.reports.extend(remote.reports);
        self.notes.extend(remote.notes);
        self.notices.extend(remote.notices);
        self.artifacts.extend(remote.artifacts);
        self.conversations.extend(remote.conversations);
        self.questions.extend(remote.questions);
        self.note_events.extend(remote.note_events);
        for status in remote.reclaim_status {
            if !self
                .reclaim_status
                .iter()
                .any(|existing| existing.terminal_id == status.terminal_id)
            {
                self.reclaim_status.push(status);
            }
        }
        self.pending_sync.extend(remote.pending_sync);
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
            self.entity_hosts.insert(tid, host);
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

    pub fn merge_remote(&mut self, mut remote: RuntimeState, host_id: &str) {
        remote.register_host(host_id);
        self.terminal_hosts.extend(remote.terminal_hosts);
        self.host_active_agents.extend(remote.host_active_agents);

        let existing_terminals: std::collections::HashSet<String> =
            self.terminals.iter().map(|t| t.id.clone()).collect();
        for t in remote.terminals {
            if !existing_terminals.contains(&t.id) {
                self.terminals.push(t);
            }
        }

        self.terminal_contexts.extend(remote.terminal_contexts);

        for status in remote.reclaim_status {
            if !self
                .reclaim_status
                .iter()
                .any(|existing| existing.terminal_id == status.terminal_id)
            {
                self.reclaim_status.push(status);
            }
        }

        for mc in remote.merging_cards {
            if !self.merging_cards.contains(&mc) {
                self.merging_cards.push(mc);
            }
        }
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
            pending_mutations: 0,
            sync_conflicts: 0,
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
            pending_mutations: 0,
            sync_conflicts: 0,
        };
        assert!(summary.is_updating());
        assert!(summary.is_offline());
        assert!(!summary.needs_attention());
    }

    #[test]
    fn runtime_state_merge_and_apply_preserve_started_at_and_non_terminal_ownership() {
        let mut local = Snapshot::default();
        local.host_summaries.insert(
            "local".into(),
            HostSummary {
                id: "local".into(),
                name: "This Mac".into(),
                is_local: true,
                transport_label: "Local socket".into(),
                endpoint_label: "/tmp/app.sock".into(),
                ssh_target: None,
                server_id: None,
                connection: HostConnectionState::Offline {
                    error: "n/a".into(),
                    cached_at: None,
                },
                active_agents: 0,
                max_agents: 4,
                project_count: 1,
                reconnect_count: 0,
                last_synced_at: None,
                pending_mutations: 0,
                sync_conflicts: 0,
            },
        );
        local.host_summaries.insert(
            "remote-1".into(),
            HostSummary {
                id: "remote-1".into(),
                name: "Remote".into(),
                is_local: false,
                transport_label: "TCP".into(),
                endpoint_label: "100.84.1.2:7680".into(),
                ssh_target: None,
                server_id: None,
                connection: HostConnectionState::Offline {
                    error: "cached".into(),
                    cached_at: Some(10),
                },
                active_agents: 0,
                max_agents: 4,
                project_count: 1,
                reconnect_count: 0,
                last_synced_at: Some(10),
                pending_mutations: 0,
                sync_conflicts: 0,
            },
        );
        // Existing terminal whose ID is shared with a scheduled run (e.g. scheduled run ID).
        local.scheduled_runs.push(
            serde_json::from_value(serde_json::json!({
                "id": "sched-shared",
                "project_id": "p1",
                "schedule_id": "s1",
                "workflow_id": "wf-1",
                "workflow_name": "Nightly",
                "filename": "nightly.yaml",
                "rule_id": "rule-1",
                "cron": "0 0 * * *",
                "number": 1,
                "trigger_kind": "cron",
                "creates_card": false,
                "state": "completed",
                "scheduled_at": 1,
                "updated_at": 1
            }))
            .unwrap(),
        );
        local
            .entity_hosts
            .insert("sched-shared".into(), "local".into());
        local
            .entity_hosts
            .insert("gone-term".into(), "local".into());
        local.terminals.push(TerminalEntry {
            id: "gone-term".into(),
            conversation_id: None,
            number: 1,
            name: Some("Gone".into()),
            origin_project_id: None,
            origin_card_id: None,
            agent_engine: None,
            runtime: None,
            agent: None,
            created_at: 1,
        });
        local.terminals.push(TerminalEntry {
            id: "sched-shared".into(),
            conversation_id: None,
            number: 2,
            name: Some("Scheduled".into()),
            origin_project_id: None,
            origin_card_id: None,
            agent_engine: None,
            runtime: None,
            agent: None,
            created_at: 1,
        });
        let existing_agent: AgentRun = serde_json::from_value(serde_json::json!({
            "project_id": "p1",
            "card_id": "c1",
            "number": 3,
            "stage": "develop",
            "trigger": "automatic",
            "profile": { "id": "developer", "enabled": true, "identity": true },
            "instruction": "Work",
            "state": "running",
            "started_at": 1_750_000_000_123u64,
            "updated_at": 100
        }))
        .unwrap();
        local.terminals.push(TerminalEntry {
            id: "agent-1".into(),
            conversation_id: Some("c1:developer".into()),
            number: 3,
            name: None,
            origin_project_id: None,
            origin_card_id: None,
            agent_engine: None,
            runtime: None,
            agent: Some(existing_agent.clone()),
            created_at: 1,
        });

        let mut incoming_agent = existing_agent;
        incoming_agent.started_at = Some(10); // Missing/sentinel timestamp in partial snapshot.
        let mut combined = RuntimeState {
            terminals: vec![TerminalEntry {
                id: "agent-1".into(),
                conversation_id: Some("c1:developer".into()),
                number: 3,
                name: None,
                origin_project_id: None,
                origin_card_id: None,
                agent_engine: None,
                runtime: None,
                agent: Some(incoming_agent),
                created_at: 1,
            }],
            host_active_agents: BTreeMap::from([("local".into(), 1)]),
            merging_cards: vec!["c1".into()],
            ..Default::default()
        };
        combined.register_host("local");

        let remote_rt = RuntimeState {
            terminals: vec![TerminalEntry {
                id: "remote-term".into(),
                conversation_id: None,
                number: 4,
                name: Some("Remote".into()),
                origin_project_id: None,
                origin_card_id: None,
                agent_engine: None,
                runtime: None,
                agent: None,
                created_at: 2,
            }],
            host_active_agents: BTreeMap::from([("remote-1".into(), 2)]),
            merging_cards: vec!["c2".into()],
            ..Default::default()
        };
        combined.merge_remote(remote_rt, "remote-1");

        local.apply_runtime_state(combined);

        assert!(!local.entity_hosts.contains_key("gone-term"));
        assert_eq!(
            local.entity_hosts.get("sched-shared").map(String::as_str),
            Some("local")
        );
        assert_eq!(
            local.entity_hosts.get("remote-term").map(String::as_str),
            Some("remote-1")
        );
        assert_eq!(local.host_summaries["local"].active_agents, 1);
        assert_eq!(local.host_summaries["remote-1"].active_agents, 2);
        let preserved = local
            .terminals
            .iter()
            .find(|t| t.id == "agent-1")
            .unwrap()
            .agent
            .as_ref()
            .unwrap();
        assert_eq!(preserved.started_at, Some(1_750_000_000_123));
        assert_eq!(local.merging_cards, vec!["c1", "c2"]);
    }
}
