//! Pure cross-Project projection. Reading/acknowledging notices never resolves a reason.
use canban_types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OverviewRow {
    pub id: String,
    pub group: String,
    pub card: Card,
    pub project_name: String,
    pub attempt: Option<Attempt>,
    pub reasons: Vec<OverviewReason>,
    pub history: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OverviewReason {
    pub source: String,
    pub message: String,
}
impl OverviewRow {
    pub fn actionable(&self) -> bool {
        !self.reasons.is_empty()
    }
    /// A follow-up invocation must not hide the result awaiting human handling.
    pub fn outcome_attempt_id(&self) -> Option<&str> {
        if self
            .reasons
            .iter()
            .any(|r| matches!(r.source.as_str(), "review" | "task"))
            && let Some(id) = self
                .card
                .human_gate
                .as_ref()
                .and_then(|g| g.attempt_id.as_deref())
        {
            return Some(id);
        }
        self.attempt.as_ref().map(|a| a.id.as_str())
    }
}

pub const GROUPS: [&str; 6] = [
    "Needs attention",
    "Ready for review",
    "Working",
    "Queued",
    "Idle",
    "Closed",
];

pub fn overview(snapshot: &Snapshot) -> Vec<OverviewRow> {
    let mut attempts: BTreeMap<String, Vec<&Attempt>> = BTreeMap::new();
    for a in &snapshot.attempts {
        attempts
            .entry(Conversation::key(&a.card_id, &a.profile.id))
            .or_default()
            .push(a);
    }
    let mut rows = Vec::new();
    for (key, mut rs) in attempts {
        rs.sort_by_key(|a| (a.created_at, a.number));
        // Retained live/uncertain invocations take precedence over queued followups.
        let run = *rs
            .iter()
            .rev()
            .find(|a| !a.finished() && (a.terminal.is_some() || a.state == "unknown"))
            .or_else(|| {
                rs.iter()
                    .rev()
                    .find(|a| matches!(a.state.as_str(), "queued" | "starting" | "unknown"))
            })
            .unwrap_or_else(|| rs.last().unwrap());
        let Some(card) = snapshot.cards.iter().find(|c| c.id == run.card_id) else {
            continue;
        };
        let mut reasons = Vec::new();
        let gate = card.human_gate.as_ref().filter(|g| {
            g.attempt_id
                .as_ref()
                .is_some_and(|id| rs.iter().any(|a| &a.id == id))
        });
        let review = gate.is_some_and(|g| {
            snapshot
                .attempts
                .iter()
                .any(|a| Some(&a.id) == g.attempt_id.as_ref() && a.state == "completed")
        }) && card.archived_at.is_none();
        if card.archived_at.is_none()
            && let Some(g) = gate
        {
            reasons.push(OverviewReason {
                source: if review { "review" } else { "task" }.into(),
                message: g.reason.clone(),
            });
        }
        let runtime = if run.terminal.is_some() || run.observed.is_some() {
            run.runtime_status()
        } else {
            "unknown"
        };
        if run.terminal.is_some() || run.observed.is_some() {
            if runtime == "blocked" {
                let msg = run
                    .lifecycle
                    .waiting
                    .as_ref()
                    .map(|w| format!("{}: {} — {}", w.kind, w.title, w.message))
                    .unwrap_or_else(|| "Agent needs terminal input".into());
                reasons.push(OverviewReason {
                    source: "runtime".into(),
                    message: msg,
                });
            }
            let awaiting_first_status = run.connection_state == "connected"
                && run
                    .observed
                    .as_ref()
                    .is_some_and(|v| v["status"]["state"].as_str().is_none())
                && now().saturating_sub(run.created_at) < 3_000;
            if !run.finished()
                && (runtime == "unknown" && run.state != "starting" && !awaiting_first_status
                    || run.close_state == "error")
            {
                reasons.push(OverviewReason {
                    source: "recovery".into(),
                    message: run.error.clone().unwrap_or_else(|| {
                        format!(
                            "Runtime status unavailable ({}) · inspect original terminal",
                            run.connection_state
                        )
                    }),
                });
            } else if runtime == "error" {
                reasons.push(OverviewReason {
                    source: "runtime".into(),
                    message: "Agent reported an error; inspect terminal".into(),
                });
            } else if !run.finished()
                && run.close_state.is_empty()
                && matches!(runtime, "idle" | "done")
                && matches!(run.state.as_str(), "running" | "working")
            {
                let msg = if run.trigger.is_automatic() {
                    "Agent stopped without reporting an outcome — inspect terminal".into()
                } else {
                    "Agent finished its turn and is waiting for your input".into()
                };
                reasons.push(OverviewReason {
                    source: "runtime".into(),
                    message: msg,
                });
            }
        } else if matches!(run.state.as_str(), "unknown" | "needs_report" | "blocked") {
            reasons.push(OverviewReason {
                source: "recovery".into(),
                message: format!("Execution {} · inspect before retrying", run.state),
            });
        }
        // An unresolved delivery must be visible independently of the Card gate.
        for q in snapshot.questions.iter().filter(|q| {
            q.card_id == card.id && q.profile_id == run.profile.id && q.state == "delivery_unknown"
        }) {
            reasons.push(OverviewReason {
                source: "question".into(),
                message: q
                    .error
                    .clone()
                    .unwrap_or_else(|| "Question delivery uncertain".into()),
            });
        }
        let group = if reasons.iter().any(|r| r.source != "review") {
            "Needs attention"
        } else if review {
            "Ready for review"
        } else if run.state == "queued" {
            "Queued"
        } else if matches!(runtime, "working" | "unknown")
            && (run.terminal.is_some() || run.observed.is_some())
            && !run.finished()
            || run.state == "starting"
        {
            "Working"
        } else if matches!(runtime, "idle" | "done") && !run.finished() {
            "Idle"
        } else {
            "Closed"
        };
        rows.push(OverviewRow {
            id: key,
            group: group.into(),
            card: card.clone(),
            project_name: snapshot
                .projects
                .iter()
                .find(|p| p.id == card.project_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| card.project_id.clone()),
            attempt: Some(run.clone()),
            reasons,
            history: rs.iter().map(|a| a.id.clone()).collect(),
        });
    }
    // Configuration/routing/recovery can fail before there is an invocation.
    for card in snapshot
        .cards
        .iter()
        .filter(|c| c.archived_at.is_none() && (c.human_gate.is_some() || c.attention.is_some()))
    {
        if rows.iter().any(|r| {
            r.card.id == card.id
                && r.reasons
                    .iter()
                    .any(|r| matches!(r.source.as_str(), "task" | "review"))
        }) {
            continue;
        }
        rows.push(OverviewRow {
            id: format!("card:{}", card.id),
            group: "Needs attention".into(),
            card: card.clone(),
            project_name: snapshot
                .projects
                .iter()
                .find(|p| p.id == card.project_id)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            attempt: None,
            history: vec![],
            reasons: vec![OverviewReason {
                source: "task".into(),
                message: card
                    .human_gate
                    .as_ref()
                    .map(|g| g.reason.clone())
                    .or(card.attention.clone())
                    .unwrap_or_default(),
            }],
        });
    }
    rows.sort_by_key(|r| {
        (
            GROUPS.iter().position(|g| *g == r.group).unwrap_or(6),
            std::cmp::Reverse(
                r.attempt
                    .as_ref()
                    .map(|a| a.created_at)
                    .unwrap_or(r.card.created_at),
            ),
            r.id.clone(),
        )
    });
    rows
}
