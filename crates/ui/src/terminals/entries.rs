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
        Some(&self.agent.as_ref()?.agent()?.profile.id)
    }
    pub fn provider(&self) -> Option<(Icon, String)> {
        if let Some(profile) = self
            .agent
            .as_ref()
            .and_then(|r| r.agent())
            .map(|a| &a.profile)
        {
            return Some(crate::profiles::provider(profile));
        }
        let engine = self.saved.agent_engine.as_deref().or_else(|| {
            let r = self.saved.runtime.as_ref()?;
            (r.state != "finished" && r.connection == "connected")
                .then_some(())
                .and(r.observed_agent.as_deref())
        })?;
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
    pub fn herdr_status(&self) -> HerdrStatus {
        self.saved.herdr_status()
    }
    pub fn runtime_wait(&self) -> Option<&RuntimeWait> {
        self.saved.runtime_wait()
    }
    pub fn wait_summary(&self) -> Option<String> {
        let w = self.runtime_wait()?;
        format_runtime_wait(w)
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
        match self.herdr_status() {
            HerdrStatus::PlainShell => "Open".into(),
            HerdrStatus::Queued => "Queued".into(),
            HerdrStatus::Starting => "Starting".into(),
            HerdrStatus::Working => "Working".into(),
            HerdrStatus::Blocked => "Blocked".into(),
            HerdrStatus::Done => "Done".into(),
            HerdrStatus::Idle => "Idle".into(),
            HerdrStatus::Error => "Error".into(),
            HerdrStatus::Disconnected => "Offline".into(),
            HerdrStatus::Closing => "Closing".into(),
            HerdrStatus::Ended => "Ended".into(),
        }
    }
    pub fn attention_reason(&self) -> Option<String> {
        if self.ended() {
            return None;
        }
        if let Some(row) = &self.agent {
            if self.herdr_status() == HerdrStatus::Blocked
                && let Some(summary) = self.wait_summary()
            {
                return Some(summary);
            }
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
        if r.connection != "connected"
            && !matches!(r.state.as_str(), "starting" | "checking" | "queued")
        {
            return Some(
                r.error
                    .clone()
                    .unwrap_or_else(|| "Terminal disconnected — inspect session".into()),
            );
        }
        if self.saved.has_active_agent() {
            match r.agent_status.as_deref() {
                Some("blocked") => {
                    return Some(
                        self.wait_summary()
                            .unwrap_or_else(|| "Agent needs terminal input".into()),
                    );
                }
                Some("error") => {
                    return Some("Agent reported an error — inspect terminal".into());
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
            let attempt = row.agent()?;
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
        if let Some(batch) = s
            .merge_batches
            .iter()
            .find(|b| b.id == self.saved.id && b.active())
        {
            let rt = self.saved.runtime.as_ref()?;
            if rt.state == "finished" || rt.state == "closing" || rt.terminal.is_none() {
                return None;
            }
            return Some(format!(
                "Merge batch Agent @{} is resolving {} Card(s). Closing this terminal cancels the batch and pauses those Cards for human review.",
                batch.profile_id,
                batch.items.iter().filter(|i| !i.evicted).count()
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
            .and_then(|g| g.terminal_id.as_ref())
            .is_some_and(|tid| Some(tid.as_str()) == row.terminal.as_ref().map(|t| t.id.as_str()));
        !waiting && matches!(self.status().as_str(), "idle" | "done")
    }
    pub fn listed(&self) -> bool {
        !self.ended() && self.card.as_ref().is_none_or(|c| c.active())
    }
    pub fn status(&self) -> String {
        if let Some(row) = &self.agent {
            row.agent()
                .map(|a| a.runtime_status())
                .unwrap_or("unknown")
                .into()
        } else if let Some(r) = &self.saved.runtime {
            if r.state == "finished" || r.state == "closing" {
                r.state.clone()
            } else if r.connection != "connected" {
                r.connection.clone()
            } else if self.saved.has_active_agent()
                && let Some(st) = &r.agent_status
            {
                st.clone()
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
            .and_then(|r| r.agent())
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
        lines.push(format!("Status: {}", self.display_status()));
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

pub fn format_runtime_wait(w: &RuntimeWait) -> Option<String> {
    let headline = match w.kind.as_str() {
        "permission" | "approval" | "command" | "tool" | "bash" | "file" => "Permission required",
        "auth" | "login" | "signin" | "sign_in" => "Sign-in required",
        "question" | "prompt" | "answer" => "Answer needed",
        _ => "Input needed",
    };
    let t = w
        .title
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let m = w
        .message
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let generic_title = t.is_empty()
        || t.eq_ignore_ascii_case("agent needs input")
        || t.eq_ignore_ascii_case("permission required")
        || t.eq_ignore_ascii_case("input required")
        || t.eq_ignore_ascii_case("sign-in required")
        || t.eq_ignore_ascii_case("answer required");
    let detail = match (generic_title, m.is_empty()) {
        (true, true) => String::new(),
        (true, false) => m,
        (false, true) => t,
        (false, false) => {
            if m.to_lowercase().contains(&t.to_lowercase()) {
                m
            } else if t.to_lowercase().contains(&m.to_lowercase()) {
                t
            } else {
                format!("{t} · {m}")
            }
        }
    };
    if detail.is_empty() {
        Some(headline.to_owned())
    } else {
        Some(format!("{headline}: {detail}"))
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
                origin_project_id: None,
                origin_card_id: None,
                agent_engine: None,
                runtime: None,
                agent: None,
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
        let merge_batch = s.merge_batches.iter().find(|b| b.id == t.id);
        let origin_card = t
            .origin_card_id
            .as_ref()
            .and_then(|id| s.cards.iter().find(|c| &c.id == id))
            .cloned();
        let in_managed_worktree = location.as_ref().is_some_and(|loc| {
            loc.worktree_id
                .as_ref()
                .is_some_and(|wid| s.worktrees.iter().any(|w| &w.id == wid))
                || loc.cwd.as_deref().is_some_and(|cwd| {
                    s.worktrees.iter().any(|w| {
                        w.location.machine == loc.machine
                            && (cwd == w.path
                                || cwd.starts_with(&format!("{}/", w.path.trim_end_matches('/'))))
                    })
                })
        });
        let card = if configuration.is_some() || merge_batch.is_some() {
            None
        } else {
            agent.as_ref().map(|r| r.card.clone()).or_else(|| {
                if in_managed_worktree {
                    return None;
                }
                location
                    .as_ref()
                    .and_then(|c| c.card_id.as_ref())
                    .and_then(|id| s.cards.iter().find(|c| &c.id == id))
                    .cloned()
                    .or_else(|| {
                        let origin = origin_card.as_ref()?;
                        let project_dir = s
                            .projects
                            .iter()
                            .find(|p| p.id == origin.project_id)
                            .map(|p| p.directory.as_str());
                        let has_dedicated_worktree = origin
                            .worktree
                            .as_deref()
                            .filter(|w| !w.is_empty() && Some(*w) != project_dir)
                            .is_some();
                        let keep_origin = match location.as_ref() {
                            None => true,
                            Some(loc) if loc.cwd.is_none() && loc.project_id.is_none() => true,
                            Some(loc) => {
                                !has_dedicated_worktree
                                    && loc.project_id.as_deref() == Some(origin.project_id.as_str())
                            }
                        };
                        keep_origin.then(|| origin.clone())
                    })
            })
        };
        let project_id = configuration
            .map(|c| c.project_id.clone())
            .or_else(|| merge_batch.map(|b| b.project_id.clone()))
            .or_else(|| {
                agent
                    .as_ref()
                    .map(|r| r.card.project_id.clone())
                    .or_else(|| location.as_ref().and_then(|c| c.project_id.clone()))
                    .or_else(|| t.origin_project_id.clone())
                    .or_else(|| card.as_ref().map(|c| c.project_id.clone()))
            });
        let inferred_card = agent.is_none() && t.origin_card_id.is_none() && card.is_some();
        let stage = agent.as_ref().and_then(|a| a.agent()).map(|att| {
            s.projects
                .iter()
                .find(|p| p.id == att.project_id)
                .and_then(|p| p.workflow.stages.iter().find(|stage| stage.id == att.stage))
                .map(|stage| stage.name.clone())
                .unwrap_or_else(|| att.stage.clone())
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
