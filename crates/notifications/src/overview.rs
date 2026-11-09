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
    pub request: Option<Request>,
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
    pub fn outcome_request_id(&self) -> Option<&str> {
        if self
            .reasons
            .iter()
            .any(|r| matches!(r.source.as_str(), "review" | "task"))
            && let Some(id) = self
                .card
                .human_gate
                .as_ref()
                .and_then(|g| g.request_id.as_deref())
        {
            return Some(id);
        }
        self.request.as_ref().map(|r| r.id.as_str())
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
    let mut requests: BTreeMap<String, Vec<&Request>> = BTreeMap::new();
    for r in &snapshot.requests {
        requests
            .entry(Conversation::key(&r.card_id, &r.profile.id))
            .or_default()
            .push(r);
    }
    let attempt = |r: &Request| {
        snapshot
            .attempts
            .iter()
            .filter(|a| a.request_id == r.id)
            .max_by_key(|a| a.number)
    };
    let mut rows = Vec::new();
    for (key, mut rs) in requests {
        rs.sort_by_key(|r| r.created_at);
        // Retained live/uncertain invocations take precedence over queued followups.
        let req = rs
            .iter()
            .rev()
            .find(|r| {
                attempt(r).is_some_and(|a| {
                    !a.finished() && (a.terminal.is_some() || a.state == "unknown")
                })
            })
            .or_else(|| {
                rs.iter()
                    .rev()
                    .find(|r| matches!(r.state.as_str(), "queued" | "starting" | "unknown"))
            })
            .unwrap_or_else(|| rs.last().unwrap());
        let Some(card) = snapshot.cards.iter().find(|c| c.id == req.card_id) else {
            continue;
        };
        let run = attempt(req);
        let mut reasons = Vec::new();
        let gate = card.human_gate.as_ref().filter(|g| {
            g.request_id
                .as_ref()
                .is_some_and(|id| rs.iter().any(|r| &r.id == id))
        });
        let review = gate.is_some_and(|g| {
            snapshot
                .requests
                .iter()
                .any(|r| Some(&r.id) == g.request_id.as_ref() && r.state == "completed")
        }) && card.archived_at.is_none();
        if card.archived_at.is_none()
            && let Some(g) = gate
        {
            reasons.push(OverviewReason {
                source: if review { "review" } else { "task" }.into(),
                message: g.reason.clone(),
            });
        }
        let runtime = run.map(|a| a.runtime_status()).unwrap_or("unknown");
        if let Some(a) = run {
            if runtime == "blocked" {
                let msg = a
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
            if !a.finished()
                && (runtime == "unknown" && a.state != "starting" || a.close_state == "error")
            {
                reasons.push(OverviewReason {
                    source: "recovery".into(),
                    message: a.error.clone().unwrap_or_else(|| {
                        format!(
                            "Runtime status unavailable ({}) · inspect original terminal",
                            a.connection_state
                        )
                    }),
                });
            } else if runtime == "error" {
                reasons.push(OverviewReason {
                    source: "runtime".into(),
                    message: "Agent reported an error; inspect terminal".into(),
                });
            }
        } else if matches!(req.state.as_str(), "unknown" | "needs_report" | "blocked") {
            reasons.push(OverviewReason {
                source: "recovery".into(),
                message: format!("Execution {} · inspect before retrying", req.state),
            });
        }
        // An unresolved delivery must be visible independently of the Card gate.
        for q in snapshot.questions.iter().filter(|q| {
            q.card_id == card.id && q.profile_id == req.profile.id && q.state == "delivery_unknown"
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
        } else if req.state == "queued" && run.is_none_or(|a| a.finished()) {
            "Queued"
        } else if matches!(runtime, "working" | "unknown") && run.is_some_and(|a| !a.finished())
            || req.state == "starting"
        {
            "Working"
        } else if matches!(runtime, "idle" | "done") {
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
            request: Some((*req).clone()),
            attempt: run.cloned(),
            reasons,
            history: snapshot
                .attempts
                .iter()
                .filter(|a| rs.iter().any(|r| a.request_id == r.id))
                .map(|a| a.id.clone())
                .collect(),
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
            request: None,
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
                r.request
                    .as_ref()
                    .map(|r| r.created_at)
                    .unwrap_or(r.card.created_at),
            ),
            r.id.clone(),
        )
    });
    rows
}
