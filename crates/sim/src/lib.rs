//! Full-scenario Deterministic Simulation Testing (DST) framework for Canban built on `turmoil`.
pub mod clock;
pub mod github;
pub mod herdr;
pub mod net;
pub mod oracle;
pub mod scenarios;
pub mod trace;
pub mod world;

pub use scenarios::{Scenario, run_scenario};

#[cfg(test)]
mod tests {
    use super::scenarios;

    #[test]
    fn scenario_1_card_automation() {
        scenarios::card_automation::run(202_601).expect("card_automation scenario failed");
    }

    #[test]
    fn scenario_2_remote_faults() {
        scenarios::remote_faults::run(202_602).expect("remote_faults scenario failed");
    }

    #[test]
    fn scenario_3_github_sync() {
        scenarios::github_sync::run(202_603).expect("github_sync scenario failed");
    }

    #[test]
    fn scenario_4_client_pages() {
        scenarios::client_pages::run(202_604).expect("client_pages scenario failed");
    }

    #[test]
    fn scenario_5_env_sync() {
        scenarios::env_sync::run(202_605).expect("env_sync scenario failed");
    }

    #[test]
    fn scenario_6_notifications() {
        scenarios::notifications::run(202_606).expect("notifications scenario failed");
    }

    #[test]
    fn scenario_7_herdr_lifecycle() {
        scenarios::herdr_lifecycle::run(202_607).expect("herdr_lifecycle scenario failed");
    }

    #[test]
    fn scenario_8_reclaim() {
        scenarios::reclaim::run(202_608).expect("reclaim scenario failed");
    }

    #[test]
    fn chaos_curated_seeds() {
        for seed in [42_u64, 1337, 202_609] {
            scenarios::chaos::run(seed, 35)
                .unwrap_or_else(|e| panic!("chaos seed {seed} failed: {e:#}"));
        }
    }

    #[test]
    fn determinism_same_seed_produces_identical_digest() {
        let d1 = scenarios::client_pages::run(777_001).expect("first run");
        let d2 = scenarios::client_pages::run(777_001).expect("second run");
        assert_eq!(
            d1, d2,
            "Determinism violation: identical seed produced different trace digests ({d1:#x} vs {d2:#x})"
        );
    }
}
