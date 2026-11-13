//! Stable navigation identity plus task association and observed location.
use super::*;
use canban_types::*;
use serde_json::{Value, json};
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub saved: TerminalEntry,
    pub agent: Option<OverviewRow>,
    pub project_id: Option<String>,
    pub project_name: String,
    pub card: Option<Card>,
    pub inferred_card: bool,
    pub location: Option<TerminalContext>,
    pub stage: Option<String>,
}
impl Entry {
    /// Automatic title is presentation only; provider/session keys never change.
    pub fn title(&self) -> String {
        self.saved.name.clone().unwrap_or_else(|| {
            if self.agent.is_some() {
                self.card
                    .as_ref()
                    .map(|c| format!("#{} {}", c.number, c.title))
                    .unwrap_or_else(|| self.saved.title())
            } else {
                self.saved.title()
            }
        })
    }
    pub fn profile(&self) -> Option<&str> {
        Some(&self.agent.as_ref()?.request.as_ref()?.profile.id)
    }
    pub fn provider(&self) -> Option<(Icon, String)> {
        if let Some(profile) = self
            .agent
            .as_ref()
            .and_then(|r| r.request.as_ref())
            .map(|r| &r.profile)
        {
            return Some(crate::profiles::provider(profile));
        }
        let engine = self.saved.agent_engine.as_deref()?;
        if let Some(tpl) = engine.strip_prefix("herdr:") {
            return Some((crate::profiles::agent_icon(tpl), format!("Herdr · {tpl}")));
        }
        Some((
            crate::profiles::agent_icon(engine),
            canban_profiles::display_name(engine)
                .map(str::to_owned)
                .unwrap_or_else(|| engine.to_owned()),
        ))
    }
    pub fn display_status(&self) -> String {
        if self.ended() {
            return "Ended".into();
        }
        if let Some(reason) = self
            .binding()
            .and_then(|b| b["input_status"].as_str().map(str::to_owned))
            .filter(|s| !s.is_empty())
        {
            return reason;
        }
        if self.needs_attention() {
            return "Needs you".into();
        }
        if self.agent.is_none() {
            return match self.status().as_str() {
                "starting" | "checking" | "connecting" => "Starting",
                "closing" => "Closing",
                "unavailable" | "replaced" => "Disconnected",
                _ => "Open",
            }
            .into();
        }
        if self.inactive() {
            return "Idle".into();
        }
        match self.status().as_str() {
            "running" | "working" => "Working",
            "queued" => "Queued",
            "starting" => "Starting",
            "idle" | "done" => "Idle",
            _ => "Unknown",
        }
        .into()
    }
    pub fn attention_reason(&self) -> Option<String> {
        if self.ended() {
            return None;
        }
        if let Some(row) = &self.agent {
            if row.actionable() {
                return Some(
                    row.reasons
                        .iter()
                        .map(|r| r.message.as_str())
                        .collect::<Vec<_>>()
                        .join(" · "),
                );
            }
            return None;
        }
        let r = self.saved.runtime.as_ref()?;
        if r.state == "finished" {
            return None;
        }
        if r.connection != "connected" && !matches!(r.state.as_str(), "starting") {
            return Some(
                r.error
                    .clone()
                    .unwrap_or_else(|| "Terminal disconnected — inspect session".into()),
            );
        }
        if self.saved.agent_engine.is_some() {
            match r.agent_status.as_deref() {
                Some("blocked") => return Some("Agent needs terminal input".into()),
                Some("error") => return Some("Agent reported an error — inspect terminal".into()),
                Some("idle" | "done") if r.agent_turn.unwrap_or(0) > 0 => {
                    return Some("Agent finished its turn and is waiting for your input".into());
                }
                _ => {}
            }
        }
        None
    }
    pub fn needs_attention(&self) -> bool {
        self.attention_reason().is_some()
    }
    /// Reason why closing this Terminal requires confirmation because a live Agent
    /// is running inside it and its status is not `done` (or `idle`).
    pub fn close_confirm_reason(&self, s: &Snapshot) -> Option<String> {
        if self.ended() {
            return None;
        }
        if let Some(row) = &self.agent {
            let attempt = row.attempt.as_ref()?;
            if attempt.finished() || attempt.terminal.is_none() || !attempt.close_state.is_empty() {
                return None;
            }
            let status = if attempt.connection_state == "connected" {
                attempt
                    .observed
                    .as_ref()
                    .and_then(|v| v["status"]["state"].as_str())
                    .unwrap_or("unknown")
            } else {
                attempt.connection_state.as_str()
            };
            if attempt.connection_state == "connected" && matches!(status, "done" | "idle") {
                return None;
            }
            let who = self
                .profile()
                .map(|p| format!("@{p}"))
                .unwrap_or_else(|| "Agent".into());
            let detail = match status {
                "working" | "running" => format!("{who} is currently running a turn."),
                "blocked" => format!("{who} is waiting for your input in this terminal."),
                "error" => format!("{who} reported an error and has not finished."),
                _ => format!("{who} is active (status `{status}`), not done."),
            };
            return Some(format!(
                "{detail} Closing this terminal terminates its live process."
            ));
        }
        if let Some(cfg) = s
            .configuration_sessions
            .iter()
            .find(|c| c.id == self.saved.id)
        {
            let rt = self.saved.runtime.as_ref()?;
            if rt.state == "finished" || rt.state == "closing" || rt.terminal.is_none() {
                return None;
            }
            let status = rt.agent_status.as_deref().unwrap_or(cfg.state.as_str());
            if rt.connection == "connected" && matches!(status, "done" | "idle") {
                return None;
            }
            return Some(format!(
                "Configuration Agent @{} is still active (not done). Closing this terminal terminates its live process.",
                cfg.profile_id
            ));
        }
        let r = self.saved.runtime.as_ref()?;
        if r.state == "finished" || r.state == "closing" || r.terminal.is_none() {
            return None;
        }
        let has_agent = self.saved.agent_engine.is_some() || r.agent_status.is_some();
        if !has_agent {
            return None;
        }
        let status = if r.connection != "connected" {
            r.connection.as_str()
        } else {
            r.agent_status.as_deref().unwrap_or("running")
        };
        if r.connection == "connected" && matches!(status, "done" | "idle") {
            return None;
        }
        let who = self
            .provider()
            .map(|(_, p)| p)
            .unwrap_or_else(|| "Agent".into());
        let detail = match status {
            "working" | "running" => format!("{who} is currently running in this terminal."),
            "blocked" => format!("{who} is waiting for your input in this terminal."),
            "error" => format!("{who} reported an error and is not done."),
            _ => format!("{who} is active in this terminal (status `{status}`), not done."),
        };
        Some(format!(
            "{detail} Closing this terminal terminates its live process."
        ))
    }
    pub fn ended(&self) -> bool {
        self.agent.as_ref().map(model::closed).unwrap_or_else(|| {
            self.saved
                .runtime
                .as_ref()
                .is_some_and(|r| r.state == "finished")
        })
    }
    /// Internal managed-Agent lifecycle state; manual terminals never participate.
    pub fn inactive(&self) -> bool {
        if self.agent.is_none() || self.needs_attention() {
            return false;
        }
        let row = self.agent.as_ref().unwrap();
        let waiting = row
            .card
            .human_gate
            .as_ref()
            .and_then(|g| g.request_id.as_ref())
            .is_some_and(|r| Some(r.as_str()) == row.request.as_ref().map(|r| r.id.as_str()));
        !waiting && matches!(self.status().as_str(), "idle" | "done")
    }
    pub fn listed(&self) -> bool {
        !self.ended() && self.card.as_ref().is_none_or(|c| c.active())
    }
    pub fn status(&self) -> String {
        if let Some(row) = &self.agent {
            row.attempt
                .as_ref()
                .map(|a| a.runtime_status())
                .unwrap_or_else(|| {
                    row.request
                        .as_ref()
                        .map(|r| r.state.as_str())
                        .unwrap_or("unknown")
                })
                .into()
        } else if let Some(r) = &self.saved.runtime {
            if r.connection != "connected" && r.state != "finished" {
                r.connection.clone()
            } else {
                r.state.clone()
            }
        } else {
            "unavailable".into()
        }
    }
    pub fn binding(&self) -> Option<Value> {
        if let Some(row) = &self.agent {
            return model::binding(row);
        }
        let r = self.saved.runtime.as_ref()?;
        (r.state != "finished").then_some(())?;
        let input_status = if r.state == "closing" {
            "Closing"
        } else if r.connection == "connecting" {
            "Connecting"
        } else if r.connection != "connected" {
            "Disconnected"
        } else {
            ""
        };
        Some(
            json!({"id":self.saved.id,"core_id":r.core_id.as_ref()?,"terminal":r.terminal.as_ref()?,"read_only":!input_status.is_empty(),"input_status":input_status}),
        )
    }
    pub fn cwd(&self) -> Option<String> {
        self.location.as_ref()?.cwd.clone()
    }
    pub fn machine(&self) -> &str {
        self.agent
            .as_ref()
            .and_then(|r| r.attempt.as_ref())
            .and_then(|a| a.location.as_ref())
            .map(|l| l.machine.as_str())
            .or_else(|| self.saved.runtime.as_ref().map(|r| r.machine.as_str()))
            .unwrap_or("local")
    }
    pub fn hint(&self) -> String {
        let context = self
            .card
            .as_ref()
            .map(|c| format!("#{} {}", c.number, c.title))
            .unwrap_or_else(|| self.project_name.clone());
        let mut lines = vec![self.title(), format!("Project: {}", self.project_name)];
        if self.agent.is_some() {
            if lines[0] != context {
                lines.push(context);
            }
            lines.push(format!(
                "{} · {} · @{}",
                self.stage.as_deref().unwrap_or("Unknown stage"),
                self.provider().map(|(_, p)| p).unwrap_or_default(),
                self.profile().unwrap_or("unknown"),
            ));
        }
        if let Some(reason) = self.attention_reason() {
            lines.push(reason);
        }
        if let Some(cwd) = self.cwd() {
            lines.push(format!("Directory: {cwd}"));
        }
        if let Some(branch) = self.location.as_ref().and_then(|c| c.branch.as_ref()) {
            lines.push(format!("Branch: {branch}"));
        }
        lines.push(format!("Machine: {}", self.machine()));
        lines.join("\n")
    }
}
pub fn entries(s: &Snapshot) -> Vec<Entry> {
    let mut agents = model::conversations(s);
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    let mut saved = s.terminals.clone();
    for row in &agents {
        if !saved
            .iter()
            .any(|t| t.conversation_id.as_deref() == Some(&row.id))
        {
            saved.push(TerminalEntry {
                id: row.id.clone(),
                conversation_id: Some(row.id.clone()),
                number: saved.iter().map(|t| t.number).max().unwrap_or(0) + 1,
                name: None,
                manual_number: None,
                origin_card_id: None,
                agent_engine: None,
                runtime: None,
                created_at: 0,
            });
        }
    }
    let mut out = Vec::new();
    for t in saved {
        let agent = t
            .conversation_id
            .as_ref()
            .and_then(|id| agents.iter().find(|r| &r.id == id))
            .cloned();
        if t.conversation_id.is_some() && agent.is_none() {
            continue;
        }
        let location = s
            .terminal_contexts
            .iter()
            .find(|c| c.terminal_id == t.id)
            .cloned();
        let configuration = s.configuration_sessions.iter().find(|c| c.id == t.id);
        let origin_card = t
            .origin_card_id
            .as_ref()
            .and_then(|id| s.cards.iter().find(|c| &c.id == id))
            .cloned();
        let card = if configuration.is_some() {
            None
        } else {
            agent
                .as_ref()
                .map(|r| r.card.clone())
                .or_else(|| {
                    location
                        .as_ref()
                        .and_then(|c| c.card_id.as_ref())
                        .and_then(|id| s.cards.iter().find(|c| &c.id == id))
                        .cloned()
                })
                .or_else(|| {
                    let origin = origin_card.as_ref()?;
                    let same_project = location.as_ref().is_none_or(|loc| {
                        (loc.project_id.is_none() && loc.cwd.is_none())
                            || loc.project_id.as_deref() == Some(origin.project_id.as_str())
                    });
                    same_project.then(|| origin.clone())
                })
        };
        let project_id = configuration.map(|c| c.project_id.clone()).or_else(|| {
            agent
                .as_ref()
                .map(|r| r.card.project_id.clone())
                .or_else(|| location.as_ref().and_then(|c| c.project_id.clone()))
                .or_else(|| card.as_ref().map(|c| c.project_id.clone()))
        });
        let inferred_card = agent.is_none() && t.origin_card_id.is_none() && card.is_some();
        let stage = agent.as_ref().and_then(|a| a.request.as_ref()).map(|r| {
            s.projects
                .iter()
                .find(|p| p.id == r.project_id)
                .and_then(|p| p.workflow.stages.iter().find(|stage| stage.id == r.stage))
                .map(|stage| stage.name.clone())
                .unwrap_or_else(|| r.stage.clone())
        });
        out.push(Entry {
            project_name: project_id
                .as_ref()
                .and_then(|id| s.projects.iter().find(|p| &p.id == id))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Other".into()),
            project_id,
            card,
            inferred_card,
            location,
            stage,
            saved: t,
            agent,
        });
    }
    out.sort_by_key(|e| e.saved.number);
    out
}
