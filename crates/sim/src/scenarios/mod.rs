//! All 8 deterministic simulation scenarios plus the full-spectrum Chaos workload.
pub mod card_automation;
pub mod chaos;
pub mod client_pages;
pub mod env_sync;
pub mod github_sync;
pub mod herdr_lifecycle;
pub mod notifications;
pub mod reclaim;
pub mod remote_faults;

use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    CardAutomation,
    RemoteFaults,
    GithubSync,
    ClientPages,
    EnvSync,
    Notifications,
    HerdrLifecycle,
    Reclaim,
    Chaos,
}

impl Scenario {
    pub const ALL_NAMED: &'static [Self] = &[
        Self::CardAutomation,
        Self::RemoteFaults,
        Self::GithubSync,
        Self::ClientPages,
        Self::EnvSync,
        Self::Notifications,
        Self::HerdrLifecycle,
        Self::Reclaim,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::CardAutomation => "card_automation",
            Self::RemoteFaults => "remote_faults",
            Self::GithubSync => "github_sync",
            Self::ClientPages => "client_pages",
            Self::EnvSync => "env_sync",
            Self::Notifications => "notifications",
            Self::HerdrLifecycle => "herdr_lifecycle",
            Self::Reclaim => "reclaim",
            Self::Chaos => "chaos",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "card_automation" | "card-automation" | "automation" => Ok(Self::CardAutomation),
            "2" | "remote_faults" | "remote-faults" | "remote" => Ok(Self::RemoteFaults),
            "3" | "github_sync" | "github-sync" | "github" => Ok(Self::GithubSync),
            "4" | "client_pages" | "client-pages" | "pages" => Ok(Self::ClientPages),
            "5" | "env_sync" | "env-sync" | "env" => Ok(Self::EnvSync),
            "6" | "notifications" | "notify" => Ok(Self::Notifications),
            "7" | "herdr_lifecycle" | "herdr-lifecycle" | "herdr" => Ok(Self::HerdrLifecycle),
            "8" | "reclaim" | "idle_reclaim" | "idle-reclaim" => Ok(Self::Reclaim),
            "chaos" | "all-chaos" => Ok(Self::Chaos),
            other => bail!(
                "Unknown scenario '{other}'. Valid: card_automation, remote_faults, github_sync, \
                 client_pages, env_sync, notifications, herdr_lifecycle, reclaim, chaos"
            ),
        }
    }
}

/// Runs a single scenario with the given deterministic seed and returns its trace digest.
pub fn run_scenario(scenario: Scenario, seed: u64, chaos_ops: usize) -> Result<u64> {
    match scenario {
        Scenario::CardAutomation => card_automation::run(seed),
        Scenario::RemoteFaults => remote_faults::run(seed),
        Scenario::GithubSync => github_sync::run(seed),
        Scenario::ClientPages => client_pages::run(seed),
        Scenario::EnvSync => env_sync::run(seed),
        Scenario::Notifications => notifications::run(seed),
        Scenario::HerdrLifecycle => herdr_lifecycle::run(seed),
        Scenario::Reclaim => reclaim::run(seed),
        Scenario::Chaos => chaos::run(seed, chaos_ops).map(|s| s.digest),
    }
}
