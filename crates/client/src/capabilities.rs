//! Runtime client capabilities and optional platform capability providers.
//!
//! Compile-time Cargo features prune desktop-only dependencies (local daemon spawning,
//! `canban-exec` SSH subprocesses, local CLI discovery, and `canban-updater`) out of mobile
//! binaries, while UI and orchestration layers query [`ClientCapabilities`] at runtime instead
//! of scattering `#[cfg(...)]` checks across presentation code.
use anyhow::Result;
use canban_types::DiscoveryCatalog;
use serde_json::Value;

/// Runtime feature matrix advertised by an active client backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClientCapabilities {
    /// Can host and manage a local `"local"` `canband` daemon over Unix sockets.
    pub local_host: bool,
    /// Can establish SSH tunnels, perform non-interactive SSH auto-pairing, and launch remote bootstrap.
    pub ssh: bool,
    /// Can enumerate local Tailscale peers (`tailscale status --json`) and `~/.ssh/config` aliases.
    pub local_tailscale_discovery: bool,
    /// Can check for and install desktop app updates (`canban-updater`).
    pub self_update: bool,
    /// Can trigger remote daemon self-updates across connected hosts.
    pub remote_daemon_update: bool,
}

impl ClientCapabilities {
    /// Pure-remote mobile preset (TCP + Tailscale `ts_auto` + `host.discover_canban`, no local subprocesses).
    pub const fn mobile() -> Self {
        Self {
            local_host: false,
            ssh: false,
            local_tailscale_discovery: false,
            self_update: false,
            remote_daemon_update: false,
        }
    }

    /// Full desktop preset (local `"local"` daemon, SSH tunnels/bootstrap, local discovery, and updater).
    pub const fn desktop() -> Self {
        Self {
            local_host: true,
            ssh: true,
            local_tailscale_discovery: true,
            self_update: true,
            remote_daemon_update: true,
        }
    }
}

/// Outcome of probing a discovered Tailscale peer and attempting direct Tailscale WhoIs auth
/// with optional non-interactive SSH auto-pair fallback.
#[derive(Clone, Debug)]
pub struct TailscalePeerConnectResult {
    pub tcp_reachable: bool,
    pub tailscale_auth_error: Option<String>,
    pub paired: std::result::Result<Value, String>,
}

/// Optional platform provider for local Tailscale / `~/.ssh/config` host discovery.
pub trait LocalDiscoveryProvider: Send + Sync + 'static {
    fn discover_local(&self) -> DiscoveryCatalog;
}

/// Optional platform provider for SSH pairing, TCP probing, and interactive remote bootstrap scripts.
pub trait SshCapabilityProvider: Send + Sync + 'static {
    fn connect_tailscale_peer_or_ssh(
        &self,
        peer_ip: &str,
        ssh_target: &str,
        port: u16,
    ) -> TailscalePeerConnectResult;

    fn pair_via_ssh(&self, ssh_target: &str, prefer_tailscale: bool, port: u16) -> Result<Value>;

    fn build_ssh_bootstrap_command(
        &self,
        ssh_target: &str,
        repo: &str,
        prefer_tailscale: bool,
        port: u16,
    ) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_presets_distinguish_mobile_and_desktop() {
        let mobile = ClientCapabilities::mobile();
        assert!(!mobile.local_host);
        assert!(!mobile.ssh);
        assert!(!mobile.local_tailscale_discovery);
        assert!(!mobile.self_update);
        assert!(!mobile.remote_daemon_update);

        let desktop = ClientCapabilities::desktop();
        assert!(desktop.local_host);
        assert!(desktop.ssh);
        assert!(desktop.local_tailscale_discovery);
        assert!(desktop.self_update);
        assert!(desktop.remote_daemon_update);
    }
}
