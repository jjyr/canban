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
        let profile = &self.agent.as_ref()?.request.as_ref()?.profile;
        Some(crate::profiles::provider(profile))
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
        if self.inactive() {
            return "Inactive".into();
        }
        match self.status().as_str() {
            "running" | "working" => "Working",
            "queued" => "Queued",
            "starting" => "Starting",
            "idle" | "done" => "Active",
            _ => "Unknown",
        }
        .into()
    }
    pub fn needs_attention(&self) -> bool {
        self.agent.as_ref().is_some_and(|r| r.actionable())
            || self
                .saved
                .runtime
                .as_ref()
                .is_some_and(|r| r.state != "finished" && r.connection != "connected")
    }
    pub fn ended(&self) -> bool {
        self.agent.as_ref().map(model::closed).unwrap_or_else(|| {
            self.saved
                .runtime
                .as_ref()
                .is_some_and(|r| r.state == "finished")
        })
    }
    /// Inactive is a live, available conversation; exited entries belong to Card history.
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
        !waiting
            && row.attempt.as_ref().is_none_or(|a| {
                a.lifecycle
                    .last_interaction_at
                    .is_none_or(|t| canban_types::now().saturating_sub(t) > 30_000)
            })
            && matches!(self.status().as_str(), "idle" | "done")
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
                "{} · {} · @{} · {}",
                self.stage.as_deref().unwrap_or("Unknown stage"),
                self.provider().map(|(_, p)| p).unwrap_or_default(),
                self.profile().unwrap_or("unknown"),
                self.display_status()
            ));
        } else {
            lines.push(self.display_status());
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
        let project_id = configuration.map(|c| c.project_id.clone()).or_else(|| {
            agent
                .as_ref()
                .map(|r| r.card.project_id.clone())
                .or_else(|| location.as_ref().and_then(|c| c.project_id.clone()))
        });
        let card = if configuration.is_some() {
            None
        } else {
            agent.as_ref().map(|r| r.card.clone()).or_else(|| {
                location
                    .as_ref()
                    .and_then(|c| c.card_id.as_ref())
                    .and_then(|id| s.cards.iter().find(|c| &c.id == id))
                    .cloned()
            })
        };
        let inferred_card = agent.is_none() && card.is_some();
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
