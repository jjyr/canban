pub mod card_automation;
pub mod chaos;
pub mod client_pages;
pub mod env_sync;
pub mod github_sync;
pub mod herdr_lifecycle;
pub mod multi_client_sync;
pub mod notifications;
pub mod project_import_export;
pub mod reclaim;
pub mod remote_faults;
pub mod review_and_events;
pub mod schedules_and_budget;
pub mod sessions_and_adoption;
pub mod transport_auth;
pub mod workflow_faults;
pub mod workspaces_delivery;

use crate::world::SimWorld;
use anyhow::{Result, bail};

/// Mix the UUID-independent canonical state digest of `world` into `world.trace`,
/// record a coverage hit, and return the final trace digest.
pub(crate) fn finalize_digest(world: &mut SimWorld) -> u64 {
    let state_digest = crate::oracle::canonical_state_digest(world);
    world.trace.mix_u64(state_digest);
    world.hit("sim.canonical_digest_mixed");
    world.trace.digest()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    CardAutomation,
    RemoteFaults,
    GitHubSync,
    ClientPages,
    EnvSync,
    HerdrLifecycle,
    Reclaim,
    Notifications,
    SchedulesAndBudget,
    WorkflowFaults,
    MultiClientSync,
    TransportAuth,
    WorkspacesDelivery,
    ProjectImportExport,
    SessionsAndAdoption,
    ReviewAndEvents,
    Chaos,
}

impl Scenario {
    pub const ALL_NAMED: &'static [Scenario] = &[
        Scenario::CardAutomation,
        Scenario::RemoteFaults,
        Scenario::GitHubSync,
        Scenario::ClientPages,
        Scenario::EnvSync,
        Scenario::HerdrLifecycle,
        Scenario::Reclaim,
        Scenario::Notifications,
        Scenario::SchedulesAndBudget,
        Scenario::WorkflowFaults,
        Scenario::MultiClientSync,
        Scenario::TransportAuth,
        Scenario::WorkspacesDelivery,
        Scenario::ProjectImportExport,
        Scenario::SessionsAndAdoption,
        Scenario::ReviewAndEvents,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            Scenario::CardAutomation => "card_automation",
            Scenario::RemoteFaults => "remote_faults",
            Scenario::GitHubSync => "github_sync",
            Scenario::ClientPages => "client_pages",
            Scenario::EnvSync => "env_sync",
            Scenario::HerdrLifecycle => "herdr_lifecycle",
            Scenario::Reclaim => "reclaim",
            Scenario::Notifications => "notifications",
            Scenario::SchedulesAndBudget => "schedules_and_budget",
            Scenario::WorkflowFaults => "workflow_faults",
            Scenario::MultiClientSync => "multi_client_sync",
            Scenario::TransportAuth => "transport_auth",
            Scenario::WorkspacesDelivery => "workspaces_delivery",
            Scenario::ProjectImportExport => "project_import_export",
            Scenario::SessionsAndAdoption => "sessions_and_adoption",
            Scenario::ReviewAndEvents => "review_and_events",
            Scenario::Chaos => "chaos",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.trim() {
            "card_automation" | "card" | "workflow" => Ok(Scenario::CardAutomation),
            "remote_faults" | "remote" | "faults" => Ok(Scenario::RemoteFaults),
            "github_sync" | "github" => Ok(Scenario::GitHubSync),
            "client_pages" | "pages" | "client" => Ok(Scenario::ClientPages),
            "env_sync" | "env" => Ok(Scenario::EnvSync),
            "herdr_lifecycle" | "herdr" => Ok(Scenario::HerdrLifecycle),
            "reclaim" | "idle" => Ok(Scenario::Reclaim),
            "notifications" | "notif" => Ok(Scenario::Notifications),
            "schedules_and_budget" | "schedules" | "budget" => Ok(Scenario::SchedulesAndBudget),
            "workflow_faults" | "wf_faults" => Ok(Scenario::WorkflowFaults),
            "multi_client_sync" | "multi_client" => Ok(Scenario::MultiClientSync),
            "transport_auth" | "auth" | "transport" => Ok(Scenario::TransportAuth),
            "workspaces_delivery" | "workspaces" | "delivery" => Ok(Scenario::WorkspacesDelivery),
            "project_import_export" | "bundle" | "import_export" => {
                Ok(Scenario::ProjectImportExport)
            }
            "sessions_and_adoption" | "sessions" | "adoption" => Ok(Scenario::SessionsAndAdoption),
            "review_and_events" | "review" | "events" => Ok(Scenario::ReviewAndEvents),
            "chaos" | "fuzz" => Ok(Scenario::Chaos),
            other => bail!(
                "Unknown scenario '{other}'. Expected one of: card_automation, remote_faults, github_sync, client_pages, env_sync, herdr_lifecycle, reclaim, notifications, schedules_and_budget, workflow_faults, multi_client_sync, transport_auth, workspaces_delivery, project_import_export, sessions_and_adoption, review_and_events, chaos"
            ),
        }
    }
}

pub fn run_scenario(scenario: Scenario, seed: u64, ops: usize) -> Result<u64> {
    match scenario {
        Scenario::CardAutomation => card_automation::run(seed),
        Scenario::RemoteFaults => remote_faults::run(seed),
        Scenario::GitHubSync => github_sync::run(seed),
        Scenario::ClientPages => client_pages::run(seed),
        Scenario::EnvSync => env_sync::run(seed),
        Scenario::HerdrLifecycle => herdr_lifecycle::run(seed),
        Scenario::Reclaim => reclaim::run(seed),
        Scenario::Notifications => notifications::run(seed),
        Scenario::SchedulesAndBudget => schedules_and_budget::run(seed),
        Scenario::WorkflowFaults => workflow_faults::run(seed),
        Scenario::MultiClientSync => multi_client_sync::run(seed),
        Scenario::TransportAuth => transport_auth::run(seed),
        Scenario::WorkspacesDelivery => workspaces_delivery::run(seed),
        Scenario::ProjectImportExport => project_import_export::run(seed),
        Scenario::SessionsAndAdoption => sessions_and_adoption::run(seed),
        Scenario::ReviewAndEvents => review_and_events::run(seed),
        Scenario::Chaos => chaos::run(seed, ops),
    }
}
