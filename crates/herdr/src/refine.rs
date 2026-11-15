//! Fine-grained agent runtime status refinement over Herdr terminal observations.
//!
//! Herdr tracks OSC 7501 status transitions (`working`, `idle`, `done`, `blocked`, `error`)
//! emitted by interactive CLI agents. Some agents (such as Antigravity `agy`) emit `idle` or
//! `done` as soon as the root prompt turn finishes, even while asynchronous Swarm workers or
//! background tasks continue running in their bottom TUI tray.
//!
//! This module provides pure, side-effect-free refinement rules keyed by canonical agent ID,
//! plus JSON projection helpers used by the observation layer after an on-demand
//! `terminal.read(lines: 0)` screen probe.

use serde_json::{Value, json};

pub mod agy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefineInput<'a> {
    pub agent_id: &'a str,
    pub raw_status: Option<&'a str>,
    pub screen_text: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefineDecision {
    /// Keep Herdr's reported status unchanged.
    Keep,
    /// Override `idle` / `done` to `working` because asynchronous background work is active.
    Working { reason: &'static str },
}

impl RefineDecision {
    pub fn is_working(self) -> bool {
        matches!(self, Self::Working { .. })
    }

    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Keep => None,
            Self::Working { reason } => Some(reason),
        }
    }
}

/// Canonicalize an agent identifier (`"agy"`, `"antigravity"`, etc.) for refinement dispatch.
pub fn canonical_agent_id(agent_id: &str) -> &str {
    match agent_id.trim() {
        "antigravity" => "agy",
        other => other,
    }
}

/// Whether this terminal observation requires an on-demand `terminal.read(lines: 0)` probe.
///
/// Refinement never runs when:
/// - the terminal has exited (`state != "live"`),
/// - the raw status is `None`, `"working"`, `"blocked"` (permission/input waits always win),
///   or `"error"`, or
/// - the active agent does not have screen-refinement rules.
pub fn needs_screen_probe(
    agent_id: Option<&str>,
    terminal_state: Option<&str>,
    raw_status: Option<&str>,
) -> bool {
    if terminal_state != Some("live") || !matches!(raw_status, Some("idle" | "done")) {
        return false;
    }
    matches!(agent_id.map(canonical_agent_id), Some("agy"))
}

/// Evaluate fine-grained status refinement from an agent's raw status and visible screen text.
pub fn refine(input: RefineInput<'_>) -> RefineDecision {
    if !matches!(input.raw_status, Some("idle" | "done")) {
        return RefineDecision::Keep;
    }
    match canonical_agent_id(input.agent_id) {
        "agy" => agy::refine_screen(input.screen_text),
        _ => RefineDecision::Keep,
    }
}

/// Project a `RefineDecision` onto a cloned Herdr terminal record before downstream observation.
///
/// When overridden to `working`, preserves the original OSC 7501 state in `status.raw_state`
/// and records the refinement rule in `status.refined`.
pub fn apply_decision(record: &mut Value, decision: RefineDecision) {
    let RefineDecision::Working { reason } = decision else {
        return;
    };
    if record["state"] != "live"
        || !matches!(record["status"]["state"].as_str(), Some("idle" | "done"))
    {
        return;
    }
    let raw_state = record["status"]["state"].clone();
    if let Some(status) = record.get_mut("status").and_then(Value::as_object_mut) {
        status.insert("raw_state".into(), raw_state);
        status.insert("state".into(), json!("working"));
        status.insert("refined".into(), json!(reason));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_gate_only_triggers_for_live_idle_or_done_agy_sessions() {
        assert!(needs_screen_probe(Some("agy"), Some("live"), Some("idle")));
        assert!(needs_screen_probe(
            Some("antigravity"),
            Some("live"),
            Some("done")
        ));

        assert!(!needs_screen_probe(Some("agy"), Some("live"), None));
        assert!(!needs_screen_probe(
            Some("agy"),
            Some("live"),
            Some("working")
        ));
        assert!(!needs_screen_probe(
            Some("agy"),
            Some("live"),
            Some("blocked")
        ));
        assert!(!needs_screen_probe(
            Some("agy"),
            Some("live"),
            Some("error")
        ));
        assert!(!needs_screen_probe(
            Some("agy"),
            Some("finished"),
            Some("done")
        ));
        assert!(!needs_screen_probe(
            Some("codex"),
            Some("live"),
            Some("idle")
        ));
        assert!(!needs_screen_probe(None, Some("live"), Some("idle")));
    }

    #[test]
    fn non_idle_or_non_agy_inputs_always_keep_raw_status() {
        let swarm_screen = "\
────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● Swarm(Demo)  worker: thinking · 2 posts · alt+s board · alt+x dismiss
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon";

        for raw in [None, Some("working"), Some("blocked"), Some("error")] {
            assert_eq!(
                refine(RefineInput {
                    agent_id: "agy",
                    raw_status: raw,
                    screen_text: swarm_screen,
                }),
                RefineDecision::Keep
            );
        }
        assert_eq!(
            refine(RefineInput {
                agent_id: "codex",
                raw_status: Some("done"),
                screen_text: swarm_screen,
            }),
            RefineDecision::Keep
        );
    }

    #[test]
    fn apply_decision_projects_working_and_retains_raw_state() {
        let mut record = json!({
            "address": "t1",
            "state": "live",
            "turn": 2,
            "status": {"state": "done", "app": "agy"}
        });
        apply_decision(
            &mut record,
            RefineDecision::Working {
                reason: "agy.swarm",
            },
        );
        assert_eq!(record["status"]["state"], "working");
        assert_eq!(record["status"]["raw_state"], "done");
        assert_eq!(record["status"]["refined"], "agy.swarm");
    }
}
