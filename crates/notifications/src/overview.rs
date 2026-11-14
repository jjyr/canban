//! Pure cross-Project projection. Reading/acknowledging notices never resolves a reason.
use canban_types::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OverviewRow {
    pub id: String,
    pub group: String,
    pub card: Card,
    pub project_name: String,
    pub terminal: Option<TerminalEntry>,
    pub reasons: Vec<OverviewReason>,
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
    pub fn agent(&self) -> Option<&AgentRun> {
        self.terminal.as_ref().and_then(|t| t.agent.as_ref())
    }
    /// A follow-up invocation must not hide the result awaiting human handling.
    pub fn outcome_terminal_id(&self) -> Option<&str> {
        if self
            .reasons
            .iter()
            .any(|r| matches!(r.source.as_str(), "review" | "task"))
            && let Some(id) = self
                .card
                .human_gate
                .as_ref()
                .and_then(|g| g.terminal_id.as_deref())
        {
            return Some(id);
        }
        self.terminal.as_ref().map(|t| t.id.as_str())
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

/// Continuous idle/done duration before an unreported automatic Agent run
/// surfaces `"Stopped before reporting"` (`1 hour`).
///
/// CLI Agents running background tasks or subagents (such as Antigravity `agy`)
/// can pause their foreground turn so Herdr observes `idle` or `done` while work
/// continues in the background. Suppressing the alert until an entire hour has
/// elapsed without status change, terminal interaction, or task report prevents
/// false positives while keeping a deterministic stall fallback.
pub const UNREPORTED_IDLE_ALERT_MS: u64 = 60 * 60 * 1_000;

/// Anchor timestamp from which an unreported automatic Agent run has remained
/// continuously in `idle` / `done` without status change, user interaction, or
/// task report.
pub fn unreported_idle_anchor(snapshot: &Snapshot, entry_id: &str, run: &AgentRun) -> Option<u64> {
    if run.finished()
        || !run.close_state.is_empty()
        || !run.trigger.is_automatic()
        || !matches!(run.state.as_str(), "running" | "working")
    {
        return None;
    }
    let runtime = if run.terminal.is_some() || run.observed.is_some() {
        run.runtime_status()
    } else {
        "unknown"
    };
    if !matches!(runtime, "idle" | "done") {
        return None;
    }
    let since = run.lifecycle.idle_since?;
    let last_report = snapshot
        .reports
        .iter()
        .filter(|r| r.terminal_id.as_deref() == Some(entry_id))
        .map(|r| r.created_at)
        .max()
        .unwrap_or(0);
    Some(
        since
            .max(run.lifecycle.last_interaction_at.unwrap_or(0))
            .max(last_report),
    )
}

pub fn unreported_idle_deadline(
    snapshot: &Snapshot,
    entry_id: &str,
    run: &AgentRun,
) -> Option<u64> {
    unreported_idle_anchor(snapshot, entry_id, run)
        .map(|anchor| anchor.saturating_add(UNREPORTED_IDLE_ALERT_MS))
}

pub fn unreported_idle_stalled(
    snapshot: &Snapshot,
    entry_id: &str,
    run: &AgentRun,
    at: u64,
) -> bool {
    unreported_idle_deadline(snapshot, entry_id, run).is_some_and(|deadline| at >= deadline)
}

pub fn overview(snapshot: &Snapshot) -> Vec<OverviewRow> {
    overview_at(snapshot, now())
}

pub fn overview_at(snapshot: &Snapshot, at: u64) -> Vec<OverviewRow> {
    let mut rows = Vec::new();
    for entry in &snapshot.terminals {
        let Some(run) = entry.agent.as_ref() else {
            continue;
        };
        let Some(card) = snapshot.cards.iter().find(|c| c.id == run.card_id) else {
            continue;
        };
        let mut reasons = Vec::new();
        let gate = card
            .human_gate
            .as_ref()
            .filter(|g| g.terminal_id.as_deref() == Some(entry.id.as_str()));
        let review = gate.is_some() && run.state == "completed" && card.archived_at.is_none();
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
                && at.saturating_sub(run.updated_at) < 3_000;
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
                if run.trigger.is_automatic() {
                    if unreported_idle_stalled(snapshot, &entry.id, run, at) {
                        reasons.push(OverviewReason {
                            source: "runtime".into(),
                            message:
                                "Agent stopped without reporting an outcome — inspect terminal"
                                    .into(),
                        });
                    }
                } else {
                    reasons.push(OverviewReason {
                        source: "runtime".into(),
                        message: "Agent finished its turn and is waiting for your input".into(),
                    });
                }
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
        let has_live_pty = (run.terminal.is_some() || run.observed.is_some()) && !run.finished();
        let group = if reasons.iter().any(|r| r.source != "review") {
            "Needs attention"
        } else if review {
            "Ready for review"
        } else if run.state == "queued" || (run.pending.is_some() && !has_live_pty) {
            "Queued"
        } else if matches!(runtime, "working" | "unknown") && has_live_pty
            || run.state == "starting"
        {
            "Working"
        } else if matches!(runtime, "idle" | "done") && !run.finished() {
            "Idle"
        } else {
            "Closed"
        };
        rows.push(OverviewRow {
            id: entry.id.clone(),
            group: group.into(),
            card: card.clone(),
            project_name: snapshot
                .projects
                .iter()
                .find(|p| p.id == card.project_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| card.project_id.clone()),
            terminal: Some(entry.clone()),
            reasons,
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
            terminal: None,
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
            std::cmp::Reverse(r.agent().map(|a| a.updated_at).unwrap_or(r.card.created_at)),
            r.id.clone(),
        )
    });
    rows
}
