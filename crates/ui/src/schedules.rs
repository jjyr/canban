//! Unified Global and Project Scheduled Tasks workspace: view, filter, create,
//! toggle, trigger (`Run now`), delete, and inspect script/agent execution history.

pub mod modal;

use crate::presenter::*;
use canban_types::{
    Card, GLOBAL_PROJECT_ID, ScheduleCardConfig, ScheduleOverlap, ScheduleSourceRef, ScheduleState,
    ScheduledRun, Snapshot, WorkflowRevision, WorkflowRule, WorkflowSource, WorkflowTemplateFile,
    cron::{CronTimeZone, describe_cron, parse_schedule},
    is_global_project_id,
};
use canban_ui_kit::{tokens::*, *};
use canban_workflow_engine::source::{self, Document, Step};
use nsh_core::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

const ALL_SCOPES_OPTION: &str = "__all__";

#[derive(Clone, Copy)]
pub struct State {
    /// Empty string = All Scopes; `GLOBAL_PROJECT_ID` = Global Host; otherwise a `Project.id`.
    pub scope: Signal<String>,
    /// `"all" | "attention" | "active" | "paused"`
    pub status_filter: Signal<String>,
    /// Canonical selection key (`<project_id>::<filename>::<rule_id>`) or `schedule_id`.
    pub selected: Signal<String>,
    /// Configuration drawer inside detail view is collapsed by default so History leads.
    pub config_expanded: Signal<bool>,
    pub artifact: Signal<Option<(String, String)>>,
    pub create_modal: Signal<bool>,
    pub modal: modal::ModalState,
    pub yaml_open: Signal<bool>,
    pub yaml_scope: Signal<String>,
    pub yaml_filename: Signal<String>,
    pub yaml_revision: Signal<String>,
    pub yaml_source: Signal<String>,
    pub yaml_error: Signal<String>,
    pub agent_open: Signal<bool>,
    pub agent_scope: Signal<String>,
    pub agent_prompt: Signal<String>,
    pub confirm_delete: Signal<Option<String>>,
    pending_toggle: Signal<Option<(String, bool)>>,
    pub error: Signal<String>,
}

impl State {
    pub fn new() -> Self {
        Self {
            scope: signal(String::new()),
            status_filter: signal("all".into()),
            selected: signal(String::new()),
            config_expanded: signal(false),
            artifact: signal(None),
            create_modal: signal(false),
            modal: modal::ModalState::new(),
            yaml_open: signal(false),
            yaml_scope: signal(String::new()),
            yaml_filename: signal(String::new()),
            yaml_revision: signal(String::new()),
            yaml_source: signal(String::new()),
            yaml_error: signal(String::new()),
            agent_open: signal(false),
            agent_scope: signal(GLOBAL_PROJECT_ID.into()),
            agent_prompt: signal(String::new()),
            confirm_delete: signal(None),
            pending_toggle: signal(None),
            error: signal(String::new()),
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduleItem {
    /// Canonical UI selection key: `<project_id>::<filename>::<rule_id>` (or `global:<host_id>::...`).
    pub key: String,
    pub legacy_key: String,
    pub schedule_id: String,
    pub project_id: String,
    pub host_id: String,
    pub scope_name: String,
    pub filename: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub workflow_enabled: bool,
    pub rule_id: String,
    pub rule: WorkflowRule,
    pub effective_enabled: bool,
    pub dirty: bool,
    pub published: bool,
    pub starts_with_script: bool,
    pub profile_id: Option<String>,
    pub agent_step_id: Option<String>,
    pub agent_instruction: String,
    pub next_run_at: Option<u64>,
    pub last_scheduled_at: Option<u64>,
    pub run_count: u64,
    pub queued_triggers: u32,
    pub latest_run: Option<ScheduledRun>,
    pub active_blocked: bool,
}

impl ScheduleItem {
    pub fn cron_str(&self) -> &str {
        self.rule.cron.as_deref().unwrap_or("").trim()
    }

    pub fn is_global(&self) -> bool {
        is_global_project_id(&self.project_id)
    }

    pub fn matches_selection(&self, sel: &str) -> bool {
        !sel.is_empty() && (self.key == sel || self.schedule_id == sel || self.legacy_key == sel)
    }

    pub fn needs_attention(&self) -> bool {
        self.active_blocked
            || self
                .latest_run
                .as_ref()
                .is_some_and(|r| matches!(r.state.as_str(), "blocked" | "failed"))
    }

    pub fn timezone_label(&self) -> String {
        CronTimeZone::parse(self.rule.timezone.as_deref())
            .map(|tz| tz.canonical_label())
            .unwrap_or_else(|_| self.rule.timezone.clone().unwrap_or_else(|| "Local".into()))
    }

    pub fn mode_badge_text(&self, stages: &[(String, String)]) -> String {
        if let Some(card_cfg) = &self.rule.card {
            let col_id = card_cfg
                .column
                .as_deref()
                .filter(|c| !c.trim().is_empty())
                .or_else(|| stages.first().map(|(id, _)| id.as_str()))
                .unwrap_or("backlog");
            let col_name = stages
                .iter()
                .find(|(id, _)| id == col_id)
                .map(|(_, name)| name.as_str())
                .unwrap_or(col_id);
            format!("Card → {col_name}")
        } else if self.starts_with_script {
            if let Some(prof) = &self.profile_id {
                format!("Script → @{prof}")
            } else {
                "Script".into()
            }
        } else if let Some(prof) = &self.profile_id {
            format!("Terminal Agent · @{prof}")
        } else {
            "Terminal Agent".into()
        }
    }
}

pub const GLOBAL_SCOPE_PREFIX: &str = "global:";

pub fn schedule_key(project_id: &str, filename: &str, rule_id: &str) -> String {
    format!("{project_id}::{filename}::{rule_id}")
}

pub fn schedule_key_for_host(
    project_id: &str,
    host_id: &str,
    filename: &str,
    rule_id: &str,
) -> String {
    if is_global_project_id(project_id) && !host_id.is_empty() && host_id != "local" {
        format!("{GLOBAL_SCOPE_PREFIX}{host_id}::{filename}::{rule_id}")
    } else {
        schedule_key(project_id, filename, rule_id)
    }
}

pub fn encode_scope_key(project_id: &str, host_id: &str) -> String {
    if is_global_project_id(project_id) {
        let h = if host_id.is_empty() { "local" } else { host_id };
        format!("{GLOBAL_SCOPE_PREFIX}{h}")
    } else {
        project_id.to_owned()
    }
}

pub fn decode_scope_key(s: &Snapshot, raw: &str) -> (String, String) {
    if let Some(hid) = raw.strip_prefix(GLOBAL_SCOPE_PREFIX) {
        let h = if hid.is_empty() { "local" } else { hid };
        (GLOBAL_PROJECT_ID.to_owned(), h.to_owned())
    } else if is_global_project_id(raw) {
        (GLOBAL_PROJECT_ID.to_owned(), "local".to_owned())
    } else {
        (raw.to_owned(), s.project_host(raw).to_owned())
    }
}

pub fn draft_key(project_id: &str, host_id: &str) -> String {
    if is_global_project_id(project_id) && !host_id.is_empty() && host_id != "local" {
        format!("{GLOBAL_SCOPE_PREFIX}{host_id}")
    } else {
        project_id.to_owned()
    }
}

pub fn scope_display_name(s: &Snapshot, project_id: &str) -> String {
    if project_id.trim().is_empty() {
        return "All Scopes".into();
    }
    if let Some(hid) = project_id.strip_prefix(GLOBAL_SCOPE_PREFIX) {
        let h = if hid.is_empty() { "local" } else { hid };
        return if s.has_remote_hosts() {
            format!("Global · {}", s.host_display_name(h))
        } else {
            "Global · Host".into()
        };
    }
    if is_global_project_id(project_id) {
        if s.has_remote_hosts() {
            format!("Global · {}", s.host_display_name("local"))
        } else {
            "Global · Host".into()
        }
    } else {
        s.projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| s.project_display_name(&p.id))
            .unwrap_or_else(|| "Project".into())
    }
}

fn find_start_mode(
    doc: &Document,
    rule: &WorkflowRule,
) -> (bool, Option<String>, Option<String>, String) {
    let Some(start) = (if rule.start_at.trim().is_empty() {
        doc.steps.first().map(|s| s.id.as_str())
    } else {
        Some(rule.start_at.trim())
    }) else {
        return (false, None, None, String::new());
    };
    let mut starts_with_script = false;
    let mut cur = Some(start.to_owned());
    let mut seen = BTreeSet::new();
    let mut first = true;
    while let Some(step_id) = cur {
        if !seen.insert(step_id.clone()) {
            break;
        }
        let Some(idx) = doc.steps.iter().position(|s| s.id == step_id) else {
            break;
        };
        let step = &doc.steps[idx];
        let kind = step.kind();
        if first && kind == "script" {
            starts_with_script = true;
        }
        first = false;
        if kind == "agent" {
            let profile = step
                .with
                .get("profile")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let instruction = step
                .with
                .get("instruction")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            return (
                starts_with_script,
                Some(step.id.clone()),
                profile,
                instruction,
            );
        }
        // Follow failure escalation targets if this is a script -> agent chain.
        cur = step
            .on_failure
            .clone()
            .or_else(|| step.next.clone())
            .or_else(|| doc.steps.get(idx + 1).map(|s| s.id.clone()));
    }
    (starts_with_script, None, None, String::new())
}

fn project_stages(ui: Ui, project_id: &str) -> Vec<(String, String)> {
    if is_global_project_id(project_id) {
        return Vec::new();
    }
    ui.snapshot.with(|s| {
        s.projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| {
                p.workflow
                    .stages
                    .iter()
                    .map(|st| (st.id.clone(), st.name.clone()))
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| {
                vec![
                    ("backlog".into(), "Backlog".into()),
                    ("develop".into(), "Develop".into()),
                    ("review".into(), "Review".into()),
                ]
            })
    })
}

fn collect_scope_items(
    s: &Snapshot,
    drafts: &BTreeMap<String, crate::workflows::Draft>,
    scope_id: &str,
    host_id: &str,
    now_ms: u64,
    items: &mut Vec<ScheduleItem>,
) {
    let dk = draft_key(scope_id, host_id);
    let draft_opt = drafts.get(&dk);
    let scope_name = if is_global_project_id(scope_id) {
        if s.has_remote_hosts() {
            format!("Global · {}", s.host_display_name(host_id))
        } else {
            "Global · Host".into()
        }
    } else {
        scope_display_name(s, scope_id)
    };
    let sources_map: BTreeMap<String, String> = if let Some(draft) = draft_opt
        && !draft.sources.is_empty()
    {
        draft.sources.clone()
    } else {
        crate::workflows::project_template_files_from_snapshot_for_host(s, scope_id, Some(host_id))
            .into_iter()
            .map(|f| (f.filename, f.source))
            .collect()
    };

    let mut covered_state_ids = BTreeSet::new();
    for (filename, raw) in &sources_map {
        let dirty = draft_opt.is_some_and(|d| crate::workflows::source_dirty(d, filename));
        let saved_src = s.workflow_sources.iter().find(|ws| {
            let ws_host = if ws.host_id.is_empty() {
                s.workflow_source_host(&ws.id)
            } else {
                ws.host_id.as_str()
            };
            ws.project_id == scope_id && ws_host == host_id && &ws.filename == filename
        });
        let Ok(doc) = source::parse(raw) else {
            continue;
        };
        let wf_id = saved_src
            .map(|ws| ws.workflow_id.clone())
            .unwrap_or_else(|| doc.id.clone());
        let wf_name = if doc.name.trim().is_empty() {
            filename.trim_end_matches(".yaml").to_owned()
        } else {
            doc.name.clone()
        };
        for (rule_id, rule) in &doc.on {
            if !rule.is_schedule() {
                continue;
            }
            let raw_sid = format!("schedule:{scope_id}:{wf_id}:{rule_id}");
            let sid = if host_id != "local" && is_global_project_id(scope_id) {
                canban_types::qualify_host_id(host_id, &raw_sid)
            } else {
                raw_sid.clone()
            };
            let persisted = s.schedule_states.iter().find(|st| {
                let st_host = if st.host_id.is_empty() {
                    s.schedule_host(&st.id)
                } else {
                    st.host_id.as_str()
                };
                st_host == host_id
                    && (st.id == sid
                        || st.id == raw_sid
                        || (st.project_id == scope_id
                            && st.filename == *filename
                            && st.rule_id == *rule_id))
            });
            if let Some(p) = persisted {
                covered_state_ids.insert(p.id.clone());
            }
            let actual_sid = persisted.map(|p| p.id.clone()).unwrap_or(sid);
            let effective_enabled = persisted
                .filter(|_| !dirty)
                .map_or(doc.enabled && rule.enabled, |state| state.enabled);
            let (starts_with_script, agent_step_id, doc_profile, agent_instruction) =
                find_start_mode(&doc, rule);
            let computed_next = if effective_enabled {
                parse_schedule(rule.cron.as_deref().unwrap_or(""), rule.timezone.as_deref())
                    .ok()
                    .and_then(|(sched, tz)| sched.next_after_ms(now_ms, tz))
            } else {
                None
            };
            let next_run_at = if !effective_enabled {
                None
            } else if dirty {
                computed_next
            } else {
                persisted.and_then(|p| p.next_run_at).or(computed_next)
            };
            let latest_run = s
                .scheduled_runs
                .iter()
                .filter(|r| {
                    let rh = if r.host_id.is_empty() {
                        s.host_of(&r.id)
                    } else {
                        r.host_id.as_str()
                    };
                    rh == host_id
                        && (r.schedule_id == actual_sid
                            || (r.project_id == scope_id
                                && r.filename == *filename
                                && r.rule_id == *rule_id))
                })
                .max_by_key(|r| (r.scheduled_at, r.number))
                .cloned();
            let active_blocked = s.scheduled_runs.iter().any(|r| {
                let rh = if r.host_id.is_empty() {
                    s.host_of(&r.id)
                } else {
                    r.host_id.as_str()
                };
                rh == host_id
                    && (r.schedule_id == actual_sid
                        || (r.project_id == scope_id
                            && r.filename == *filename
                            && r.rule_id == *rule_id))
                    && r.state == "blocked"
            });

            items.push(ScheduleItem {
                key: schedule_key_for_host(scope_id, host_id, filename, rule_id),
                legacy_key: format!("{filename}::{rule_id}"),
                schedule_id: actual_sid,
                project_id: scope_id.to_owned(),
                host_id: host_id.to_owned(),
                scope_name: scope_name.clone(),
                filename: (*filename).clone(),
                workflow_id: wf_id.clone(),
                workflow_name: wf_name.clone(),
                workflow_enabled: doc.enabled,
                rule_id: rule_id.clone(),
                rule: rule.clone(),
                effective_enabled,
                dirty,
                published: persisted.is_some() && !dirty,
                starts_with_script: persisted.map_or(starts_with_script, |p| {
                    starts_with_script || p.starts_with_script
                }),
                profile_id: doc_profile.or_else(|| persisted.and_then(|p| p.profile_id.clone())),
                agent_step_id,
                agent_instruction,
                next_run_at,
                last_scheduled_at: persisted.and_then(|p| p.last_scheduled_at),
                run_count: persisted.map_or(0, |p| p.run_count),
                queued_triggers: persisted.map_or(0, |p| p.queued_triggers),
                latest_run,
                active_blocked,
            });
        }
    }

    // Fallback for any ScheduleState present in Snapshot whose WorkflowSource isn't mapped yet.
    for st in s.schedule_states.iter().filter(|st| {
        let st_host = if st.host_id.is_empty() {
            s.schedule_host(&st.id)
        } else {
            st.host_id.as_str()
        };
        st.project_id == scope_id && st_host == host_id && !covered_state_ids.contains(&st.id)
    }) {
        let latest_run = s
            .scheduled_runs
            .iter()
            .filter(|r| {
                let rh = if r.host_id.is_empty() {
                    s.host_of(&r.id)
                } else {
                    r.host_id.as_str()
                };
                rh == host_id && r.schedule_id == st.id
            })
            .max_by_key(|r| (r.scheduled_at, r.number))
            .cloned();
        let active_blocked = s.scheduled_runs.iter().any(|r| {
            let rh = if r.host_id.is_empty() {
                s.host_of(&r.id)
            } else {
                r.host_id.as_str()
            };
            rh == host_id && r.schedule_id == st.id && r.state == "blocked"
        });
        items.push(ScheduleItem {
            key: schedule_key_for_host(scope_id, host_id, &st.filename, &st.rule_id),
            legacy_key: format!("{}::{}", st.filename, st.rule_id),
            schedule_id: st.id.clone(),
            project_id: st.project_id.clone(),
            host_id: host_id.to_owned(),
            scope_name: scope_name.clone(),
            filename: st.filename.clone(),
            workflow_id: st.workflow_id.clone(),
            workflow_name: st.workflow_name.clone(),
            workflow_enabled: true,
            rule_id: st.rule_id.clone(),
            rule: WorkflowRule {
                event: "schedule".into(),
                cron: Some(st.cron.clone()),
                timezone: st.timezone.clone(),
                overlap: st.overlap,
                card: st.creates_card.then(|| ScheduleCardConfig {
                    column: st.target_column.clone(),
                    title: None,
                    body: None,
                }),
                enabled: st.enabled,
                ..Default::default()
            },
            effective_enabled: st.enabled,
            dirty: false,
            published: true,
            starts_with_script: st.starts_with_script,
            profile_id: st.profile_id.clone(),
            agent_step_id: None,
            agent_instruction: String::new(),
            next_run_at: st.next_run_at,
            last_scheduled_at: st.last_scheduled_at,
            run_count: st.run_count,
            queued_triggers: st.queued_triggers,
            latest_run,
            active_blocked,
        });
    }
}

fn ordered_global_hosts(s: &Snapshot) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut hosts = Vec::new();
    seen.insert("local".to_owned());
    hosts.push("local".to_owned());

    let mut remotes: Vec<(&str, &str)> = s
        .host_summaries
        .iter()
        .filter(|(id, _)| id.as_str() != "local")
        .map(|(id, h)| (h.name.as_str(), id.as_str()))
        .collect();
    remotes.sort_by(|a, b| {
        a.0.to_ascii_lowercase()
            .cmp(&b.0.to_ascii_lowercase())
            .then_with(|| a.1.cmp(b.1))
    });
    for (_, hid) in remotes {
        if seen.insert(hid.to_owned()) {
            hosts.push(hid.to_owned());
        }
    }
    for st in &s.schedule_states {
        if is_global_project_id(&st.project_id) {
            let h = if st.host_id.is_empty() {
                s.schedule_host(&st.id)
            } else {
                st.host_id.as_str()
            };
            if seen.insert(h.to_owned()) {
                hosts.push(h.to_owned());
            }
        }
    }
    for ws in &s.workflow_sources {
        if is_global_project_id(&ws.project_id) {
            let h = if ws.host_id.is_empty() {
                s.workflow_source_host(&ws.id)
            } else {
                ws.host_id.as_str()
            };
            if seen.insert(h.to_owned()) {
                hosts.push(h.to_owned());
            }
        }
    }
    hosts
}

/// Collects scheduled tasks for Global and/or Project scopes across hosts.
pub fn collect_schedules(ui: Ui, scope_filter: &str) -> Vec<ScheduleItem> {
    let drafts = ui.workflows.drafts.get();
    let pending_toggle = ui.schedules.pending_toggle.get();
    ui.snapshot.with(|s| {
        let now_ms = canban_types::now();
        let mut items = Vec::new();
        if scope_filter.is_empty() {
            for hid in ordered_global_hosts(s) {
                collect_scope_items(s, &drafts, GLOBAL_PROJECT_ID, &hid, now_ms, &mut items);
            }
            for p in &s.projects {
                let ph = s.project_host(&p.id);
                collect_scope_items(s, &drafts, &p.id, ph, now_ms, &mut items);
            }
        } else if let Some(target_hid) = scope_filter.strip_prefix(GLOBAL_SCOPE_PREFIX) {
            let hid = if target_hid.is_empty() {
                "local"
            } else {
                target_hid
            };
            collect_scope_items(s, &drafts, GLOBAL_PROJECT_ID, hid, now_ms, &mut items);
        } else if is_global_project_id(scope_filter) {
            for hid in ordered_global_hosts(s) {
                collect_scope_items(s, &drafts, GLOBAL_PROJECT_ID, &hid, now_ms, &mut items);
            }
        } else {
            let ph = s.project_host(scope_filter);
            collect_scope_items(s, &drafts, scope_filter, ph, now_ms, &mut items);
        }
        if let Some((id, enabled)) = pending_toggle.as_ref()
            && let Some(item) = items
                .iter_mut()
                .find(|item| item.schedule_id == *id && item.published)
        {
            item.effective_enabled = *enabled;
            item.next_run_at = enabled
                .then(|| {
                    parse_schedule(item.cron_str(), item.rule.timezone.as_deref())
                        .ok()
                        .and_then(|(schedule, tz)| schedule.next_after_ms(now_ms, tz))
                })
                .flatten();
        }
        items.sort_by(|a, b| {
            b.needs_attention()
                .cmp(&a.needs_attention())
                .then_with(|| b.is_global().cmp(&a.is_global()))
                .then_with(|| a.scope_name.cmp(&b.scope_name))
                .then_with(|| a.workflow_name.cmp(&b.workflow_name))
                .then_with(|| a.rule_id.cmp(&b.rule_id))
        });
        items
    })
}

/// Collects all scheduled rules across the currently active Project (`ui.project`).
pub fn collect_project_schedules(ui: Ui) -> Vec<ScheduleItem> {
    let project_id = ui.project.get();
    if project_id.is_empty() {
        return Vec::new();
    }
    collect_schedules(ui, &project_id)
}

pub fn project_schedule_count(ui: Ui) -> usize {
    collect_project_schedules(ui).len()
}

pub fn attention_count(ui: Ui) -> usize {
    collect_schedules(ui, "")
        .into_iter()
        .filter(ScheduleItem::needs_attention)
        .count()
}

pub fn open(ui: Ui, scope: Option<String>, select_schedule: Option<String>) {
    batch(move || {
        ui.schedules.scope.set(scope.unwrap_or_default());
        ui.schedules.status_filter.set("all".into());
        ui.schedules.config_expanded.set(false);
        ui.schedules.confirm_delete.set(None);
        if let Some(sel) = select_schedule {
            ui.schedules.selected.set(sel);
        }
        if ui.page.get_untracked() != "schedules" {
            ui.navigate("schedules");
        }
    });
}

pub fn open_schedules(ui: Ui, select_schedule: Option<String>) {
    let scope = select_schedule
        .as_deref()
        .and_then(|sel| {
            collect_schedules(ui, "")
                .into_iter()
                .find(|i| i.matches_selection(sel))
                .map(|i| encode_scope_key(&i.project_id, &i.host_id))
        })
        .or_else(|| {
            let pid = ui.project.get_untracked();
            (!pid.is_empty()).then_some(pid)
        });
    open(ui, scope, select_schedule);
}

fn ensure_draft_initialized(ui: Ui, scope_key: &str) {
    let state = ui.workflows;
    let has = state.drafts.with_untracked(|d| d.contains_key(scope_key));
    if has {
        return;
    }
    let files: Vec<WorkflowTemplateFile> = ui.snapshot.with_untracked(|s| {
        let (actual_pid, target_hid) = decode_scope_key(s, scope_key);
        crate::workflows::project_template_files_from_snapshot_for_host(
            s,
            &actual_pid,
            Some(&target_hid),
        )
    });
    if files.is_empty() {
        return;
    }
    let base_sources: Vec<_> = files
        .iter()
        .map(|f| json!({"filename": f.filename, "source": f.source}))
        .collect();
    state.drafts.update(|d| {
        d.entry(scope_key.to_owned())
            .or_default()
            .accept(json!({"revision": "local", "sources": base_sources}));
    });
}

pub fn new_scheduled_workflow(ui: Ui) {
    let scope = if ui.page.get_untracked() == "schedules" {
        let cur = ui.schedules.scope.get_untracked();
        (!cur.is_empty()).then_some(cur)
    } else {
        let pid = ui.project.get_untracked();
        (!pid.is_empty()).then_some(pid)
    };
    modal::open_create_modal(ui, scope);
}

pub fn add_schedule_rule_to_file(ui: Ui, filename: &str) {
    let project = ui.project.get_untracked();
    if project.is_empty() || filename.is_empty() {
        return;
    }
    ensure_draft_initialized(ui, &project);
    let default_prof = crate::sessions::default_profile(ui);
    let mut added_rule_id = String::new();
    mutate_workflow_file(ui, &project, filename, |doc| {
        let mut idx = 1usize;
        let mut rule_id = "schedule".to_owned();
        while doc.on.contains_key(&rule_id) {
            idx += 1;
            rule_id = format!("schedule_{idx}");
        }
        let has_stages = !doc.columns.is_empty()
            || doc
                .steps
                .iter()
                .any(|s| matches!(s.kind(), "card.move" | "deliver" | "approval" | "admit"));
        let (start_at, card_cfg) = if has_stages {
            let first_col = doc
                .columns
                .first()
                .map(|c| c.id.clone())
                .unwrap_or_else(|| "backlog".into());
            let entry = doc
                .column_entry(&first_col)
                .map(|(_, step)| step)
                .or_else(|| doc.steps.first().map(|s| s.id.clone()))
                .unwrap_or_else(|| {
                    let step_id = "agent:scheduled".to_owned();
                    let mut with = BTreeMap::new();
                    if !default_prof.is_empty() {
                        with.insert("profile".into(), json!(default_prof.clone()));
                    }
                    doc.steps.insert(
                        0,
                        Step {
                            id: step_id.clone(),
                            name: Some("Scheduled Agent".into()),
                            uses: "agent".into(),
                            with,
                            ..Default::default()
                        },
                    );
                    step_id
                });
            (
                entry,
                Some(ScheduleCardConfig {
                    column: Some(first_col),
                    title: Some(format!("{} · {{{{date}}}}", doc.name)),
                    body: None,
                }),
            )
        } else {
            let entry = doc
                .steps
                .iter()
                .find(|s| s.kind() == "agent" || s.kind() == "script")
                .map(|s| s.id.clone())
                .unwrap_or_else(|| {
                    let step_id = "agent:run".to_owned();
                    let mut with = BTreeMap::new();
                    if !default_prof.is_empty() {
                        with.insert("profile".into(), json!(default_prof.clone()));
                    }
                    with.insert(
                        "instruction".into(),
                        json!("Run the scheduled maintenance check and report findings."),
                    );
                    doc.steps.insert(
                        0,
                        Step {
                            id: step_id.clone(),
                            name: Some("Scheduled Agent".into()),
                            uses: "agent".into(),
                            with,
                            ..Default::default()
                        },
                    );
                    step_id
                });
            (entry, None)
        };
        doc.on.insert(
            rule_id.clone(),
            WorkflowRule {
                event: "schedule".into(),
                start_at,
                cron: Some("0 9 * * 1-5".into()),
                card: card_cfg,
                enabled: true,
                ..Default::default()
            },
        );
        added_rule_id = rule_id;
    });
    if !added_rule_id.is_empty() {
        open(
            ui,
            Some(project.clone()),
            Some(schedule_key(&project, filename, &added_rule_id)),
        );
    }
}

pub(crate) fn mutate_workflow_file(
    ui: Ui,
    project: &str,
    filename: &str,
    mutate: impl FnOnce(&mut Document),
) {
    let state = ui.workflows;
    if project.is_empty() || filename.is_empty() {
        return;
    }
    ensure_draft_initialized(ui, project);
    let Some(raw) = state.drafts.with_untracked(|d| {
        d.get(project)
            .and_then(|dr| dr.sources.get(filename))
            .cloned()
    }) else {
        return;
    };
    let Ok(mut doc) = source::parse(&raw) else {
        return;
    };
    mutate(&mut doc);
    let Ok(encoded) = source::encode(&doc) else {
        return;
    };
    if state.selected.get_untracked() == filename {
        state
            .studio_synced
            .replace((project.to_owned(), filename.to_owned(), encoded.clone()));
        state.studio_error.set(String::new());
        state.studio_doc.set(Some(doc));
    }
    state.drafts.update(|d| {
        if let Some(dr) = d.get_mut(project) {
            dr.sources.insert(filename.to_owned(), encoded);
        }
    });
}

fn toggle_gallery_schedule(
    ui: Ui,
    project: &str,
    filename: &str,
    rule_id: &str,
    sid: &str,
    enabled: bool,
) {
    mutate_workflow_file(ui, project, filename, |doc| {
        if enabled {
            doc.enabled = true;
        }
        if let Some(rule) = doc.on.get_mut(rule_id) {
            rule.enabled = enabled;
        }
    });
    let now_ms = canban_types::now();
    ui.workflows.drafts.update(|d| {
        if let Some(dr) = d.get_mut(project) {
            dr.base = json!({
                "revision": "gallery",
                "sources": dr.inputs(),
            });
        }
    });
    ui.snapshot.update(|s| {
        if let Some(st) = s.schedule_states.iter_mut().find(|st| st.id == sid) {
            st.enabled = enabled;
            st.next_run_at = if enabled {
                parse_schedule(&st.cron, st.timezone.as_deref())
                    .ok()
                    .and_then(|(sched, tz)| sched.next_after_ms(now_ms, tz))
            } else {
                None
            };
            st.updated_at = now_ms;
        }
    });
}

pub fn toggle_schedule(ui: Ui, item: &ScheduleItem, enabled: bool) {
    let state = ui.workflows;
    let project = item.project_id.clone();
    let host_id = item.host_id.clone();
    let dk = draft_key(&project, &host_id);
    let filename = item.filename.clone();
    let rule_id = item.rule_id.clone();

    let draft_dirty = state
        .drafts
        .with_untracked(|d| d.get(&dk).is_some_and(crate::workflows::Draft::needs_apply));

    // If there are unsaved draft edits, or this schedule has not been published yet,
    // mutate the draft directly so the user's unsaved edits are never clobbered.
    if draft_dirty || !item.published {
        mutate_workflow_file(ui, &dk, &filename, |doc| {
            if enabled {
                doc.enabled = true;
            }
            if let Some(rule) = doc.on.get_mut(&rule_id) {
                rule.enabled = enabled;
            }
        });
        return;
    }

    let sid = item.schedule_id.clone();
    if !crate::backend::is_connected() {
        toggle_gallery_schedule(ui, &dk, &filename, &rule_id, &sid, enabled);
        ui.notify(if enabled {
            "Schedule enabled"
        } else {
            "Schedule paused"
        });
        return;
    }

    if ui.is_method_pending("schedule.toggle") {
        return;
    }
    ui.schedules
        .pending_toggle
        .set(Some((sid.clone(), enabled)));
    dispatch_complete(
        ui,
        "schedule.toggle",
        json!({
            "schedule_id": sid,
            "host_id": host_id,
            "enabled": enabled,
        }),
        move |res| {
            ui.schedules.pending_toggle.set(None);
            match res {
                Ok(_) => {
                    if ui.project.get_untracked() == project {
                        crate::workflows::load(ui, false);
                    }
                    ui.notify(if enabled {
                        "Schedule enabled"
                    } else {
                        "Schedule paused"
                    });
                }
                Err(e) => {
                    ui.schedules.error.set(e);
                    ui.operation_error.set(String::new());
                }
            }
        },
    );
}

pub fn delete_schedule(ui: Ui, item: &ScheduleItem) {
    let project = item.project_id.clone();
    let host_id = item.host_id.clone();
    let dk = draft_key(&project, &host_id);
    let filename = item.filename.clone();
    let rule_id = item.rule_id.clone();
    let sid = item.schedule_id.clone();
    ui.schedules.confirm_delete.set(None);

    if !crate::backend::is_connected() || !item.published {
        let mut empty_doc = false;
        mutate_workflow_file(ui, &dk, &filename, |doc| {
            doc.on.remove(&rule_id);
            empty_doc = doc.on.is_empty();
        });
        ui.workflows.drafts.update(|d| {
            if let Some(dr) = d.get_mut(&dk)
                && empty_doc
                && (is_global_project_id(&project) || dr.sources.len() > 1)
            {
                dr.sources.remove(&filename);
                if dr.selected == filename {
                    dr.selected = dr.sources.keys().next().cloned().unwrap_or_default();
                }
            }
        });
        ui.snapshot.update(|s| {
            s.schedule_states.retain(|st| st.id != sid);
            if empty_doc {
                s.workflow_sources.retain(|ws| {
                    !(ws.project_id == project && ws.filename == filename && ws.host_id == host_id)
                });
            }
        });
        ui.notify("Scheduled task removed");
        return;
    }

    dispatch_complete(
        ui,
        "schedule.delete",
        json!({
            "schedule_id": sid,
            "project_id": project,
            "host_id": host_id,
            "filename": filename,
            "rule_id": rule_id,
        }),
        move |res| match res {
            Ok(_) => {
                ui.schedules.selected.set(String::new());
                crate::workflows::load(ui, true);
                ui.notify("Scheduled task deleted");
            }
            Err(e) => {
                ui.schedules.error.set(e);
                ui.operation_error.set(String::new());
            }
        },
    );
}

pub fn trigger_schedule(ui: Ui, item: &ScheduleItem) {
    let state = ui.workflows;
    let project = item.project_id.clone();
    let dk = draft_key(&project, &item.host_id);
    let has_unsaved = state
        .drafts
        .with_untracked(|d| d.get(&dk).is_some_and(crate::workflows::Draft::needs_apply));
    if has_unsaved || !item.published {
        ui.notify(format!(
            "Save workflow changes first ({}) before running this schedule",
            crate::commands::shortcuts::SAVE_WORKFLOW_DISPLAY
        ));
        return;
    }

    if !crate::backend::is_connected() {
        let now_ms = canban_types::now();
        let sid = item.schedule_id.clone();
        let wf_id = item.workflow_id.clone();
        let wf_name = item.workflow_name.clone();
        let filename = item.filename.clone();
        let rule_id = item.rule_id.clone();
        let cron = item.cron_str().to_owned();
        let prof = item.profile_id.clone();
        let card_cfg = item.rule.card.clone();
        let tz = CronTimeZone::parse(item.rule.timezone.as_deref()).unwrap_or(CronTimeZone::Local);
        let (date, dt) = tz.format_timestamp(now_ms);
        ui.snapshot.update(|s| {
            let next_num = s
                .scheduled_runs
                .iter()
                .filter(|r| r.schedule_id == sid)
                .map(|r| r.number)
                .max()
                .unwrap_or(0)
                + 1;
            let short = canban_types::id();
            let run_id = format!("scheduled:{project}:{short}");
            let (creates_card, card_id, card_number, terminal_id, summary) =
                if let Some(cfg) = card_cfg {
                    let cnum = s
                        .cards
                        .iter()
                        .filter(|c| c.project_id == project)
                        .map(|c| c.number)
                        .max()
                        .unwrap_or(0)
                        + 1;
                    let cid = format!("card-sched-{cnum}");
                    let col = cfg.column.unwrap_or_else(|| "backlog".into());
                    let title = cfg
                        .title
                        .unwrap_or_else(|| format!("{wf_name} · {{{{date}}}}"))
                        .replace("{{date}}", &date)
                        .replace("{{datetime}}", &dt)
                        .replace("{{run_number}}", &next_num.to_string())
                        .replace("{{workflow}}", &wf_name)
                        .replace("{{rule}}", &rule_id);
                    let body = cfg
                        .body
                        .unwrap_or_default()
                        .replace("{{date}}", &date)
                        .replace("{{datetime}}", &dt)
                        .replace("{{run_number}}", &next_num.to_string())
                        .replace("{{workflow}}", &wf_name)
                        .replace("{{rule}}", &rule_id);
                    s.cards.push(Card {
                        schedule_source: Some(ScheduleSourceRef {
                            schedule_id: sid.clone(),
                            workflow_id: wf_id.clone(),
                            workflow_name: wf_name.clone(),
                            filename: filename.clone(),
                            rule_id: rule_id.clone(),
                            cron: cron.clone(),
                            run_id: run_id.clone(),
                            run_number: next_num,
                            scheduled_at: now_ms,
                        }),
                        source_trust: None,
                        hold_auto_merge: false,
                        github_links: Vec::new(),
                        created_by: format!("schedule:{wf_name}"),
                        assignee: None,
                        content: Default::default(),
                        removed_at: None,
                        pull_request: None,
                        pull_request_history: Vec::new(),
                        delivery_mode: None,
                        workspace: None,
                        workspace_history: Vec::new(),
                        id: cid.clone(),
                        project_id: project.clone(),
                        number: cnum,
                        title,
                        body,
                        board_order: None,
                        context_notes: None,
                        stage: col,
                        revision: 1,
                        attention: None,
                        archived_at: None,
                        archive: None,
                        archive_error: None,
                        worktree: None,
                        branch: None,
                        base_commit: None,
                        human_gate: None,
                        delivery: None,
                        resolution: None,
                        created_at: now_ms,
                    });
                    (
                        true,
                        Some(cid),
                        Some(cnum),
                        None,
                        Some(format!("Created Card #{cnum} via manual trigger")),
                    )
                } else {
                    (
                        false,
                        None,
                        None,
                        Some(run_id.clone()),
                        Some("Launched Terminal Agent via manual trigger".into()),
                    )
                };
            s.scheduled_runs.insert(
                0,
                ScheduledRun {
                    host_id: item.host_id.clone(),
                    id: run_id.clone(),
                    schedule_id: sid.clone(),
                    project_id: project.clone(),
                    workflow_id: wf_id,
                    workflow_name: wf_name,
                    filename,
                    rule_id,
                    cron,
                    number: next_num,
                    trigger_kind: "manual".into(),
                    creates_card,
                    card_id,
                    card_number,
                    workflow_run_id: None,
                    start_step: Some(item.rule.start_at.clone()),
                    current_step: Some(item.rule.start_at.clone()),
                    profile_id: prof,
                    provider_session_id: None,
                    terminal_id,
                    cwd: String::new(),
                    state: "running".into(),
                    summary,
                    report_path: None,
                    output_path: None,
                    error: None,
                    steps: Vec::new(),
                    scheduled_at: now_ms,
                    started_at: Some(now_ms),
                    completed_at: None,
                    close_requested_at: None,
                    updated_at: now_ms,
                },
            );
            if let Some(st) = s.schedule_states.iter_mut().find(|st| st.id == sid) {
                st.run_count += 1;
                st.last_run_id = Some(run_id);
                st.last_scheduled_at = Some(now_ms);
                st.updated_at = now_ms;
            }
        });
        ui.notify("Scheduled task triggered");
        return;
    }

    let sid = item.schedule_id.clone();
    let host_id = item.host_id.clone();
    dispatch_complete(
        ui,
        "schedule.trigger",
        json!({ "schedule_id": sid, "host_id": host_id }),
        move |res| match res {
            Ok(v) => {
                if let Some(cnum) = v["card_number"].as_u64() {
                    ui.notify(format!("Triggered schedule · Created Card #{cnum}"));
                } else {
                    ui.notify("Scheduled task triggered");
                }
            }
            Err(e) => {
                ui.schedules.error.set(e);
                ui.operation_error.set(String::new());
            }
        },
    );
}

pub fn trigger_schedule_by_id(ui: Ui, schedule_id: &str) {
    let items = collect_schedules(ui, "");
    if let Some(item) = items.iter().find(|i| i.matches_selection(schedule_id)) {
        trigger_schedule(ui, item);
        return;
    }
    if crate::backend::is_connected() {
        dispatch(
            ui,
            "schedule.trigger",
            json!({ "schedule_id": schedule_id }),
        );
    }
}

pub fn toggle_schedule_by_id(ui: Ui, schedule_id: &str) {
    let items = collect_schedules(ui, "");
    if let Some(item) = items.iter().find(|i| i.matches_selection(schedule_id)) {
        toggle_schedule(ui, item, !item.effective_enabled);
        return;
    }
    if let Some((host_id, enabled)) = ui.snapshot.with_untracked(|s| {
        s.schedule_states
            .iter()
            .find(|st| st.id == schedule_id)
            .map(|st| {
                let h = if st.host_id.is_empty() {
                    s.schedule_host(&st.id)
                } else {
                    st.host_id.as_str()
                };
                (h.to_owned(), st.enabled)
            })
    }) && crate::backend::is_connected()
    {
        dispatch(
            ui,
            "schedule.toggle",
            json!({ "schedule_id": schedule_id, "host_id": host_id, "enabled": !enabled }),
        );
    }
}

pub(crate) fn offline_save_schedules(ui: Ui, project_id: &str) {
    let items = collect_schedules(ui, project_id);
    let now_ms = canban_types::now();
    let stages = project_stages(ui, project_id);
    ui.snapshot.update(|s| {
        s.schedule_states.retain(|st| st.project_id != project_id);
        for item in items {
            let cron = item.cron_str().to_owned();
            if cron.is_empty() {
                continue;
            }
            let target_column = item.rule.card.as_ref().map(|c| {
                c.column
                    .clone()
                    .filter(|col| !col.trim().is_empty())
                    .or_else(|| stages.first().map(|(id, _)| id.clone()))
                    .unwrap_or_else(|| "backlog".into())
            });
            let next_run_at = if item.effective_enabled {
                parse_schedule(&cron, item.rule.timezone.as_deref())
                    .ok()
                    .and_then(|(sched, tz)| sched.next_after_ms(now_ms, tz))
            } else {
                None
            };
            s.schedule_states.push(ScheduleState {
                host_id: item.host_id,
                id: item.schedule_id,
                project_id: project_id.to_owned(),
                workflow_id: item.workflow_id,
                workflow_name: item.workflow_name,
                filename: item.filename,
                rule_id: item.rule_id,
                cron,
                timezone: item.rule.timezone,
                overlap: item.rule.overlap,
                creates_card: item.rule.card.is_some(),
                starts_with_script: item.starts_with_script,
                target_column,
                profile_id: item.profile_id,
                enabled: item.effective_enabled,
                last_scheduled_at: item.last_scheduled_at,
                next_run_at,
                last_run_id: None,
                run_count: item.run_count,
                queued_triggers: item.queued_triggers,
                updated_at: now_ms,
            });
        }
    });
}

pub(crate) fn format_run_ts(ts: u64, tz_raw: Option<&str>) -> String {
    let tz = CronTimeZone::parse(tz_raw).unwrap_or(CronTimeZone::Local);
    tz.format_timestamp(ts).1
}

fn run_tone(state: &str) -> Tone {
    match state {
        "completed" => Tone::Success,
        "running" | "working" | "starting" | "queued" => Tone::Info,
        "blocked" | "skipped" | "unknown" => Tone::Attention,
        "failed" => Tone::Danger,
        _ => Tone::Neutral,
    }
}

fn run_label(state: &str) -> &'static str {
    match state {
        "completed" => "Completed",
        "running" | "working" => "Running",
        "starting" => "Starting",
        "queued" => "Queued",
        "blocked" => "Needs you",
        "skipped" => "Skipped",
        "failed" => "Failed",
        "cancelled" => "Cancelled",
        "unknown" => "Interrupted",
        _ => "Finished",
    }
}

fn open_run_artifact(ui: Ui, run: &ScheduledRun, path: Option<&str>, title: String) {
    let s = ui.schedules;
    if !crate::backend::is_connected() || path.is_none() {
        let body = run
            .summary
            .clone()
            .or_else(|| run.error.clone())
            .unwrap_or_else(|| "No report text was recorded for this scheduled run.".into());
        s.artifact.set(Some((title, body)));
        return;
    }
    let run_id = run.id.clone();
    let path = path.unwrap().to_owned();
    let fallback = run.summary.clone().unwrap_or_default();
    ui.spawn(async move {
        let res = blocking(move || {
            crate::routed_call(
                "schedule.read_artifact",
                json!({ "schedule_run_id": run_id, "path": path }),
                None,
            )
        })
        .await;
        match res {
            Ok(Ok(v)) => {
                let body = v["body"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| fallback.clone());
                s.artifact.set(Some((title, body)));
            }
            Ok(Err(e)) => {
                if !fallback.is_empty() {
                    s.artifact.set(Some((title, fallback)));
                } else {
                    s.error.set(format!("{e:#}"));
                }
            }
            Err(e) => s.error.set(e.to_string()),
        }
    });
}

fn filter_matches(item: &ScheduleItem, status_filter: &str) -> bool {
    match status_filter {
        "attention" => item.needs_attention(),
        "active" => item.effective_enabled,
        "paused" => !item.effective_enabled,
        _ => true,
    }
}

pub fn chrome_bar(ui: Ui) -> View {
    let s = ui.schedules;
    let scope_label = computed(move || {
        let scope = s.scope.get();
        ui.snapshot.with(|snap| scope_display_name(snap, &scope))
    });
    let attention_total = computed(move || attention_count(ui));
    let all_items = computed(move || collect_schedules(ui, ""));

    let scope_picker = search_popover(
        press_target()
            .h(px(28.))
            .px_2p5()
            .rounded(px(6.))
            .border_1()
            .border_color(BORDER)
            .bg(INSET)
            .hover(|st| st.bg(CARD_HOVER))
            .focus(|st| st.border_color(ACCENT))
            .active(|st| st.opacity(0.75))
            .flex_row()
            .items_center()
            .gap_1p5()
            .accessibility_label(move || {
                Some(format!("Schedule scope: {}", scope_label.get()).into())
            })
            .child(icon(Icon::Projects).w(px(13.)).h(px(13.)))
            .child(
                label(move || scope_label.get())
                    .text_size(px(CAPTION_SIZE))
                    .font_weight(FontWeight::MEDIUM)
                    .max_w(px(180.))
                    .truncate(),
            )
            .child(icon(Icon::ChevronDown).w(px(11.)).h(px(11.))),
        move || {
            let cur = s.scope.get();
            let list = all_items.get();
            let total_n = list.len();
            ui.snapshot.with(|snap| {
                let (cur_pid, cur_hid) = decode_scope_key(snap, &cur);
                let has_remote = snap.has_remote_hosts();
                let mut opts = Vec::with_capacity(snap.projects.len() + 4);
                opts.push(
                    PickerOption::new(ALL_SCOPES_OPTION, "All Scopes")
                        .detail(format!("{total_n} total"))
                        .icon(Icon::Projects)
                        .selected(cur.is_empty()),
                );
                for hid in ordered_global_hosts(snap) {
                    let gn = list
                        .iter()
                        .filter(|i| i.is_global() && i.host_id == hid)
                        .count();
                    let opt_val = if has_remote {
                        encode_scope_key(GLOBAL_PROJECT_ID, &hid)
                    } else {
                        GLOBAL_PROJECT_ID.to_owned()
                    };
                    let opt_label = if has_remote {
                        format!("Global · {}", snap.host_display_name(&hid))
                    } else {
                        "Global · Host".to_owned()
                    };
                    let selected = !cur.is_empty()
                        && is_global_project_id(&cur_pid)
                        && (cur_hid == hid || !has_remote);
                    let mut opt = PickerOption::new(opt_val, opt_label)
                        .detail(format!("{gn} host tasks"))
                        .icon(Icon::Server)
                        .selected(selected);
                    if has_remote {
                        opt = opt.group("Global · Hosts");
                    }
                    opts.push(opt);
                }
                for p in &snap.projects {
                    let pn = list.iter().filter(|i| i.project_id == p.id).count();
                    opts.push(
                        PickerOption::new(p.id.clone(), snap.project_display_name(&p.id))
                            .group("Projects")
                            .detail(format!("{pn} scheduled"))
                            .icon(Icon::Folder)
                            .selected(cur == p.id),
                    );
                }
                opts
            })
        },
        move |picked| {
            if picked == ALL_SCOPES_OPTION {
                s.scope.set(String::new());
            } else {
                s.scope.set(picked);
            }
        },
    )
    .placeholder("Filter Schedules by Scope…");

    let status_seg = segmented()
        .child(compact_segment(
            "All",
            None,
            move || s.status_filter.get() == "all",
            move |_| s.status_filter.set("all".into()),
        ))
        .child(
            compact_segment(
                "Attention",
                None,
                move || s.status_filter.get() == "attention",
                move |_| s.status_filter.set("attention".into()),
            )
            .child(when(
                move || attention_total.get() > 0,
                move || reminder_count(move || attention_total.get(), || true),
            )),
        )
        .child(compact_segment(
            "Active",
            None,
            move || s.status_filter.get() == "active",
            move |_| s.status_filter.set("active".into()),
        ))
        .child(compact_segment(
            "Paused",
            None,
            move || s.status_filter.get() == "paused",
            move |_| s.status_filter.set("paused".into()),
        ));

    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex_row()
        .items_center()
        .gap_2()
        .pl_2()
        .window_drag(true)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.))
                .window_drag(false)
                .child(icon_tinted(Icon::Clock, ACCENT).w(px(15.)).h(px(15.)))
                .child(
                    label("Schedules")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(13.5)),
                )
                .child(scope_picker)
                .child(status_seg),
        )
        .child(view().flex_1().h_full().window_drag(true))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .window_drag(false)
                .child(compact_action(
                    "Describe with agent",
                    Variant::Secondary,
                    move |_| {
                        let scope = s.scope.get_untracked();
                        modal::describe_with_agent(ui, scope, None);
                    },
                ))
                .child(action_with_icon(
                    Icon::Plus,
                    "New schedule",
                    Variant::Primary,
                    move |_| new_scheduled_workflow(ui),
                )),
        )
}

pub fn screen(ui: Ui) -> AnyElement {
    let s = ui.schedules;
    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .p_5()
        .gap_3()
        .child(when(
            move || !s.error.get().is_empty(),
            move || notice(move || s.error.get(), Tone::Attention),
        ))
        .child(render(ui))
        .into_element()
}

pub fn render(ui: Ui) -> AnyElement {
    let s = ui.schedules;
    let scoped_items = computed(move || {
        let scope = s.scope.get();
        collect_schedules(ui, &scope)
    });
    let filtered_items = computed(move || {
        let status = s.status_filter.get();
        scoped_items
            .get()
            .into_iter()
            .filter(|it| filter_matches(it, &status))
            .collect::<Vec<_>>()
    });

    effect(move || {
        let list = filtered_items.get();
        let cur = s.selected.get_untracked();
        if list.is_empty() {
            return;
        }
        if !list.iter().any(|i| i.matches_selection(&cur)) {
            s.selected.set(list[0].key.clone());
        }
    });

    let selected_item = computed(move || {
        let cur = s.selected.get();
        filtered_items
            .get()
            .into_iter()
            .find(|i| i.matches_selection(&cur))
    });

    let selected_dirty = computed(move || selected_item.get().is_some_and(|i| i.dirty));

    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .gap_3()
        .child(when(
            move || selected_dirty.get(),
            move || {
                let item = selected_item.get();
                let dk = item
                    .as_ref()
                    .map(|i| draft_key(&i.project_id, &i.host_id))
                    .unwrap_or_default();
                let scope_lbl = item
                    .as_ref()
                    .map(|i| i.scope_name.clone())
                    .unwrap_or_default();
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .p_2p5()
                    .rounded(px(RADIUS))
                    .bg(SURFACE)
                    .border_1()
                    .border_color(ACCENT)
                    .child(muted(format!(
                        "Unsaved configuration edits for {scope_lbl}. Save before running."
                    )))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(compact_action("Discard", Variant::Quiet, {
                                let p = dk.clone();
                                move |_| {
                                    ui.workflows.drafts.update(|d| {
                                        d.remove(&p);
                                    });
                                }
                            }))
                            .child(compact_action("Save Changes", Variant::Primary, {
                                let p = dk;
                                move |_| save_scope_draft(ui, &p)
                            })),
                    )
            },
        ))
        .child(when(
            move || filtered_items.with(Vec::is_empty),
            move || empty_state(ui),
        ))
        .child(when(
            move || !filtered_items.with(Vec::is_empty),
            move || {
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .flex_row()
                    .gap_4()
                    .child(master_list(ui, filtered_items))
                    .child(dynamic(move || {
                        if let Some(item) = selected_item.get() {
                            let stages = project_stages(ui, &item.project_id);
                            detail_pane(ui, item, stages)
                        } else {
                            view()
                                .flex_1()
                                .items_center()
                                .justify_center()
                                .child(muted("Select a scheduled task"))
                                .into_element()
                        }
                    }))
            },
        ))
        .child(when(
            move || s.artifact.get().is_some(),
            move || artifact_dialog(ui),
        ))
        .child(when(
            move || s.create_modal.get(),
            move || modal::create_modal(ui),
        ))
        .child(when(
            move || s.yaml_open.get(),
            move || modal::yaml_modal(ui),
        ))
        .child(when(
            move || s.agent_open.get(),
            move || modal::agent_modal(ui),
        ))
        .into_element()
}

fn save_scope_draft(ui: Ui, scope_key: &str) {
    let (actual_pid, target_hid) = ui
        .snapshot
        .with_untracked(|s| decode_scope_key(s, scope_key));
    if !crate::backend::is_connected() {
        offline_save_schedules(ui, &actual_pid);
        ui.workflows.drafts.update(|d| {
            if let Some(dr) = d.get_mut(scope_key) {
                dr.base = json!({ "revision": "gallery", "sources": dr.inputs() });
            }
        });
        ui.notify("Schedules saved");
        return;
    }
    let Some(inputs) = ui
        .workflows
        .drafts
        .with_untracked(|d| d.get(scope_key).map(crate::workflows::Draft::inputs))
    else {
        return;
    };
    let pid = actual_pid;
    let hid = target_hid;
    let remove_pid = scope_key.to_owned();
    ui.spawn(async move {
        let res = blocking(move || {
            let col = crate::routed_call(
                "workflow.sources",
                json!({"project_id": pid, "host_id": hid}),
                None,
            )?;
            let rev = col["revision"].as_str().unwrap_or("").to_owned();
            crate::routed_call(
                "workflow.apply",
                json!({
                    "project_id": pid,
                    "host_id": hid,
                    "revision": rev,
                    "sources": inputs,
                    "request_key": canban_types::id(),
                }),
                None,
            )
        })
        .await;
        match res {
            Ok(Ok(_)) => {
                ui.workflows.drafts.update(|d| {
                    d.remove(&remove_pid);
                });
                ui.notify("Scheduled workflow published");
            }
            Ok(Err(e)) => ui.schedules.error.set(format!("{e:#}")),
            Err(e) => ui.schedules.error.set(e.to_string()),
        }
    });
}

fn empty_state(ui: Ui) -> impl IntoElement {
    let scope = ui.schedules.scope.get();
    let status = ui.schedules.status_filter.get();
    panel()
        .flex_1()
        .min_h(px(0.))
        .items_center()
        .justify_center()
        .gap_3()
        .border_1()
        .border_color(BORDER)
        .child(tone_icon_badge(Icon::Clock, Tone::Info, false))
        .child(heading(if status == "attention" {
            "No Schedules Need Attention"
        } else {
            "No Scheduled Tasks Configured"
        }))
        .child(
            muted(if status == "attention" {
                "No scheduled runs are currently blocked waiting for input or failed."
            } else {
                "Create Global host automation (like 7-day merged worktree cleanup) or Project-scoped cron workflows from here."
            })
            .max_w(px(520.)),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(action_with_icon(
                    Icon::Plus,
                    "New scheduled workflow",
                    Variant::Primary,
                    move |_| new_scheduled_workflow(ui),
                ))
                .child(when(
                    move || !scope.is_empty() && !is_global_project_id(&scope),
                    move || {
                        compact_action("Show All Scopes", Variant::Secondary, move |_| {
                            ui.schedules.scope.set(String::new());
                        })
                    },
                )),
        )
}

fn master_list(ui: Ui, items: Memo<Vec<ScheduleItem>>) -> impl IntoElement {
    let s = ui.schedules;
    view()
        .w(px(330.))
        .flex_shrink_0()
        .min_h(px(0.))
        .gap_2p5()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    label(move || format!("Scheduled Tasks ({})", items.with(Vec::len)))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(compact_action(
                    "+ New schedule",
                    Variant::Secondary,
                    move |_| new_scheduled_workflow(ui),
                )),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .child(view().gap_2().pr_1().child(each(
                    move || items.get(),
                    |item| {
                        format!(
                            "{}:{}:{}:{}",
                            item.key,
                            item.effective_enabled,
                            item.active_blocked,
                            item.latest_run.as_ref().map_or("", |r| r.state.as_str())
                        )
                    },
                    move |item_sig, _| {
                        let key = item_sig.with_untracked(|i| i.key.clone());
                        let sid = item_sig.with_untracked(|i| i.schedule_id.clone());
                        let lkey = item_sig.with_untracked(|i| i.legacy_key.clone());
                        let select_key = key.clone();
                        let card_key = key.clone();
                        let card_sid = sid.clone();
                        let card_lkey = lkey;
                        let is_selected = computed(move || {
                            let cur = s.selected.get();
                            cur == card_key || cur == card_sid || cur == card_lkey
                        });
                        let item_stages =
                            project_stages(ui, &item_sig.with_untracked(|i| i.project_id.clone()));

                        press_target()
                            .w_full()
                            .p_3()
                            .gap_2()
                            .rounded(px(RADIUS))
                            .border_1()
                            .border_color(move || if is_selected.get() { ACCENT } else { BORDER })
                            .bg(move || if is_selected.get() { SELECTED } else { SURFACE })
                            .hover(|st| st.border_color(ACCENT))
                            .selected(move || is_selected.get())
                            .accessibility_label(move || {
                                item_sig.with(|it| {
                                    Some(
                                        format!(
                                            "{} · {} · {} · {}",
                                            it.scope_name,
                                            it.workflow_name,
                                            it.cron_str(),
                                            if it.effective_enabled {
                                                "Enabled"
                                            } else {
                                                "Paused"
                                            }
                                        )
                                        .into(),
                                    )
                                })
                            })
                            .on_press(move || {
                                s.confirm_delete.set(None);
                                s.selected.set(select_key.clone());
                            })
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_1p5()
                                    .min_w(px(0.))
                                    .child(badge(move || {
                                        let has_remote =
                                            ui.snapshot.with(|snap| snap.has_remote_hosts());
                                        item_sig.with(|i| {
                                            if i.is_global() && !has_remote {
                                                "Global".to_owned()
                                            } else {
                                                i.scope_name.clone()
                                            }
                                        })
                                    }))
                                    .child(
                                        label(move || {
                                            item_sig.with(|i| {
                                                let suffix = if i.dirty { " •" } else { "" };
                                                if i.rule_id == "schedule" {
                                                    format!("{}{suffix}", i.workflow_name)
                                                } else {
                                                    format!(
                                                        "{} / {}{suffix}",
                                                        i.workflow_name, i.rule_id
                                                    )
                                                }
                                            })
                                        })
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .truncate()
                                        .flex_1()
                                        .min_w(px(0.)),
                                    )
                                    .child({
                                        #[derive(Clone, Copy, PartialEq, Eq)]
                                        enum RowStatus {
                                            NeedsYou,
                                            Running,
                                            Failed,
                                            Active,
                                            Paused,
                                        }
                                        let row_status = computed(move || {
                                            let enabled = item_sig.with(|i| i.effective_enabled);
                                            let blocked = item_sig.with(|i| i.active_blocked);
                                            let latest_failed = item_sig.with(|i| {
                                                i.latest_run
                                                    .as_ref()
                                                    .is_some_and(|r| r.state == "failed")
                                            });
                                            let active_run = ui.snapshot.with(|snap| {
                                                let s_id = item_sig.with(|i| i.schedule_id.clone());
                                                snap.scheduled_runs
                                                    .iter()
                                                    .any(|r| r.schedule_id == s_id && r.active())
                                            });
                                            if blocked {
                                                RowStatus::NeedsYou
                                            } else if active_run {
                                                RowStatus::Running
                                            } else if latest_failed {
                                                RowStatus::Failed
                                            } else if enabled {
                                                RowStatus::Active
                                            } else {
                                                RowStatus::Paused
                                            }
                                        });
                                        dynamic(move || match row_status.get() {
                                            RowStatus::NeedsYou => {
                                                live_status("Needs you", Tone::Attention)
                                                    .into_element()
                                            }
                                            RowStatus::Running => {
                                                live_status("Running", Tone::Info).into_element()
                                            }
                                            RowStatus::Failed => {
                                                status("Failed", Tone::Danger).into_element()
                                            }
                                            RowStatus::Active => {
                                                status("Active", Tone::Success).into_element()
                                            }
                                            RowStatus::Paused => {
                                                status("Paused", Tone::Neutral).into_element()
                                            }
                                        })
                                    }),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_1p5()
                                    .child(badge(move || {
                                        item_sig.with(|i| {
                                            let c = i.cron_str();
                                            if c.is_empty() {
                                                "(no cron)".into()
                                            } else {
                                                c.to_owned()
                                            }
                                        })
                                    }))
                                    .child(badge({
                                        let stg = item_stages.clone();
                                        move || item_sig.with(|i| i.mode_badge_text(&stg))
                                    })),
                            )
                            .child(
                                muted(move || {
                                    item_sig.with(|i| {
                                        let desc = describe_cron(i.cron_str());
                                        if let Some(next_ms) = i.next_run_at {
                                            let next_str =
                                                format_run_ts(next_ms, i.rule.timezone.as_deref());
                                            format!("{desc} · Next: {next_str}")
                                        } else if !i.effective_enabled {
                                            format!("{desc} · Paused")
                                        } else {
                                            desc
                                        }
                                    })
                                })
                                .truncate(),
                            )
                    },
                ))),
        )
}

fn detail_pane(ui: Ui, item: ScheduleItem, stages: Vec<(String, String)>) -> AnyElement {
    let filename = item.filename.clone();
    let rule_id = item.rule_id.clone();
    let schedule_id = item.schedule_id.clone();
    let item_for_toggle = item.clone();
    let item_for_run = item.clone();
    let item_for_del = item.clone();

    let cron_validation = parse_schedule(item.cron_str(), item.rule.timezone.as_deref());
    let cron_error = cron_validation.as_ref().err().cloned();
    let cron_desc = describe_cron(item.cron_str());
    let next_preview = cron_validation
        .ok()
        .and_then(|(s, tz)| s.next_after_ms(canban_types::now(), tz))
        .map(|ts| format_run_ts(ts, item.rule.timezone.as_deref()));

    let active_terminal_id = ui.snapshot.with(|s| {
        s.scheduled_runs
            .iter()
            .find(|r| r.schedule_id == schedule_id && r.active())
            .and_then(|r| r.terminal_id.clone())
    });
    let blocked_run = ui.snapshot.with(|s| {
        s.scheduled_runs
            .iter()
            .find(|r| r.schedule_id == schedule_id && r.state == "blocked")
            .cloned()
    });

    let runs = computed({
        let sid = schedule_id.clone();
        let pid = item.project_id.clone();
        let fname = filename.clone();
        let rid = rule_id.clone();
        move || {
            ui.snapshot.with(|s| {
                let mut list: Vec<ScheduledRun> = s
                    .scheduled_runs
                    .iter()
                    .filter(|r| {
                        r.schedule_id == sid
                            || (r.project_id == pid && r.filename == fname && r.rule_id == rid)
                    })
                    .cloned()
                    .collect();
                list.sort_by(|a, b| {
                    b.scheduled_at
                        .cmp(&a.scheduled_at)
                        .then_with(|| b.number.cmp(&a.number))
                });
                list
            })
        }
    });

    let item_cfg = item.clone();
    let cfg_desc = cron_desc.clone();
    let cfg_next = next_preview.clone();
    let cfg_cron_label = format!(
        "{} · {} · {}",
        item.cron_str(),
        item.timezone_label(),
        cron_desc
    );

    scroll()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .child(
            view()
                .gap_3()
                .pb_4()
                .child(detail_header_card(
                    ui,
                    &item,
                    item_for_toggle,
                    item_for_run,
                    item_for_del,
                    active_terminal_id,
                    &cron_desc,
                    next_preview.as_deref(),
                ))
                .child(if let Some(br) = blocked_run {
                    let msg = br.summary.clone().unwrap_or_else(|| {
                        "This scheduled run is waiting for your input in Terminal.".into()
                    });
                    let tid = br.terminal_id.unwrap_or_else(|| br.id.clone());
                    panel()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .border_1()
                        .border_color(ACCENT)
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .min_w(px(0.))
                                .flex_1()
                                .child(tone_icon_badge(Icon::CirclePause, Tone::Attention, true))
                                .child(
                                    view()
                                        .gap_0p5()
                                        .min_w(px(0.))
                                        .flex_1()
                                        .child(
                                            label(format!("Run #{} needs your input", br.number))
                                                .font_weight(FontWeight::SEMIBOLD),
                                        )
                                        .child(muted(msg)),
                                ),
                        )
                        .child(action_with_icon(
                            Icon::Terminal,
                            "Open Live Terminal →",
                            Variant::Primary,
                            move |_| ui.open_terminal(tid.clone()),
                        ))
                        .into_element()
                } else {
                    view().into_element()
                })
                .child(history_panel_section(ui, &item, runs))
                .child(
                    view()
                        .gap_2()
                        .child(
                            press_target()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .p_2p5()
                                .rounded(px(RADIUS))
                                .bg(SURFACE)
                                .border_1()
                                .border_color(BORDER)
                                .hover(|st| st.border_color(ACCENT))
                                .on_press(move || {
                                    ui.schedules
                                        .config_expanded
                                        .set(!ui.schedules.config_expanded.get_untracked());
                                })
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(icon(Icon::Edit).w(px(14.)).h(px(14.)))
                                        .child(
                                            label("Configuration & Schedule Rules")
                                                .font_weight(FontWeight::SEMIBOLD),
                                        )
                                        .child(muted(cfg_cron_label)),
                                )
                                .child(label(move || {
                                    if ui.schedules.config_expanded.get() {
                                        "▾ Hide"
                                    } else {
                                        "▸ Edit"
                                    }
                                })),
                        )
                        .child(when(move || ui.schedules.config_expanded.get(), {
                            let item_inner = item_cfg.clone();
                            let stages_inner = stages.clone();
                            let err_inner = cron_error.clone();
                            let desc_inner = cfg_desc.clone();
                            let next_inner = cfg_next.clone();
                            move || {
                                config_panel(
                                    ui,
                                    &item_inner,
                                    &stages_inner,
                                    err_inner.clone(),
                                    desc_inner.clone(),
                                    next_inner.clone(),
                                )
                            }
                        })),
                ),
        )
        .into_element()
}

#[allow(clippy::too_many_arguments)]
fn detail_header_card(
    ui: Ui,
    item: &ScheduleItem,
    item_for_toggle: ScheduleItem,
    item_for_run: ScheduleItem,
    item_for_del: ScheduleItem,
    active_terminal_id: Option<String>,
    cron_desc: &str,
    next_preview: Option<&str>,
) -> impl IntoElement {
    let filename = item.filename.clone();
    let filename_yaml = item.filename.clone();
    let filename_studio = item.filename.clone();
    let scope_id_yaml = encode_scope_key(&item.project_id, &item.host_id);
    let scope_id_studio = item.project_id.clone();
    let is_global = item.is_global();
    let host_offline = ui.snapshot.with(|s| s.is_host_offline(&item.host_id));
    let host_display = ui
        .snapshot
        .with(|s| s.host_display_name(&item.host_id).to_owned());
    let enabled = item.effective_enabled;
    let wf_name = item.workflow_name.clone();
    let rule_id = item.rule_id.clone();
    let dirty = item.dirty;
    let published = item.published;
    let queued = item.queued_triggers;
    let run_count = item.run_count;
    let sid_del = item.schedule_id.clone();
    let sid_del_confirm = item.schedule_id.clone();

    let subtitle = if let Some(next_str) = next_preview {
        format!(
            "{cron_desc} · Next: {next_str} ({}) · {run_count} total run{}{}",
            item.timezone_label(),
            if run_count == 1 { "" } else { "s" },
            if queued > 0 { " · 1 queued" } else { "" }
        )
    } else {
        format!(
            "{cron_desc} · {run_count} total run{}{}",
            if run_count == 1 { "" } else { "s" },
            if queued > 0 { " · 1 queued" } else { "" }
        )
    };

    panel()
        .gap_2p5()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .min_w(px(0.))
                        .child(tone_icon_badge(
                            Icon::Clock,
                            if item.active_blocked {
                                Tone::Attention
                            } else if enabled {
                                Tone::Info
                            } else {
                                Tone::Neutral
                            },
                            item.active_blocked,
                        ))
                        .child(
                            view()
                                .gap_0p5()
                                .min_w(px(0.))
                                .child(
                                    view()
                                        .flex_row()
                                        .flex_wrap()
                                        .items_center()
                                        .gap_2()
                                        .child(badge(item.scope_name.clone()))
                                        .child(heading(if rule_id == "schedule" {
                                            wf_name.clone()
                                        } else {
                                            format!("{wf_name} / {rule_id}")
                                        }))
                                        .child(badge(filename))
                                        .child(if host_offline {
                                            status(format!("{host_display} offline"), Tone::Attention)
                                        } else if dirty {
                                            status("Unsaved draft", Tone::Attention)
                                        } else if enabled {
                                            status("Enabled", Tone::Success)
                                        } else {
                                            status("Paused", Tone::Neutral)
                                        }),
                                )
                                .child(muted(subtitle)),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(if let Some(tid) = active_terminal_id {
                            compact_action("Live Terminal →", Variant::Secondary, move |_| {
                                ui.open_terminal(tid.clone());
                            })
                            .into_element()
                        } else {
                            view().into_element()
                        })
                        .child(
                            compact_action(
                                if enabled { "Pause" } else { "Enable" },
                                Variant::Secondary,
                                move |_| {
                                    toggle_schedule(ui, &item_for_toggle, !enabled);
                                },
                            )
                            .disabled(move || host_offline),
                        )
                        .child(
                            action_with_icon(Icon::Zap, "Run now", Variant::Primary, move |_| {
                                trigger_schedule(ui, &item_for_run)
                            })
                            .disabled(move || {
                                host_offline
                                    || ui.is_method_pending("schedule.trigger")
                                    || dirty
                                    || !published
                            }),
                        )
                        .child(
                            compact_action("Edit YAML", Variant::Quiet, move |_| {
                                modal::open_yaml_editor(
                                    ui,
                                    scope_id_yaml.clone(),
                                    filename_yaml.clone(),
                                );
                            })
                            .disabled(move || host_offline),
                        )
                        .child(if !is_global {
                            compact_action("Open Studio", Variant::Quiet, move |_| {
                                ui.project.set(scope_id_studio.clone());
                                ui.workflows.drafts.update(|d| {
                                    if let Some(dr) = d.get_mut(&scope_id_studio) {
                                        dr.selected = filename_studio.clone();
                                    }
                                });
                                ui.workflows.selected.set(filename_studio.clone());
                                ui.workflows.mode.set("visual".into());
                                ui.navigate("workflows");
                            })
                            .into_element()
                        } else {
                            view().into_element()
                        })
                        .child(dynamic(move || {
                            let confirming = ui
                                .schedules
                                .confirm_delete
                                .with(|c| c.as_deref() == Some(sid_del.as_str()));
                            if confirming {
                                let del_item = item_for_del.clone();
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        compact_action(
                                            "Confirm Delete",
                                            Variant::Danger,
                                            move |_| delete_schedule(ui, &del_item),
                                        )
                                        .disabled(move || host_offline),
                                    )
                                    .child(compact_action("Cancel", Variant::Quiet, move |_| {
                                        ui.schedules.confirm_delete.set(None);
                                    }))
                                    .into_element()
                            } else {
                                let s_click = sid_del_confirm.clone();
                                compact_action("Delete", Variant::Quiet, move |_| {
                                    ui.schedules.confirm_delete.set(Some(s_click.clone()));
                                })
                                .disabled(move || host_offline)
                                .into_element()
                            }
                        })),
                ),
        )
        .child(when(
            move || host_offline,
            move || {
                notice(
                    format!("Machine '{host_display}' is offline. Reconnect to edit or run this schedule."),
                    Tone::Attention,
                )
            },
        ))
}

fn config_panel(
    ui: Ui,
    item: &ScheduleItem,
    stages: &[(String, String)],
    cron_error: Option<String>,
    cron_desc: String,
    next_preview: Option<String>,
) -> AnyElement {
    let pid = draft_key(&item.project_id, &item.host_id);
    let filename = item.filename.clone();
    let rule_id = item.rule_id.clone();
    let cur_cron = item.cron_str().to_owned();
    let cur_tz = item
        .rule
        .timezone
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_owned();
    let cur_overlap = item.rule.overlap;
    let creates_card = item.rule.card.is_some();
    let wf_name_val = item.workflow_name.clone();

    let pid_name = pid.clone();
    let fname_name = filename.clone();
    let pid_cron = pid.clone();
    let fname_cron = filename.clone();
    let rid_cron = rule_id.clone();
    let pid_tz = pid.clone();
    let fname_tz = filename.clone();
    let rid_tz = rule_id.clone();
    let pid_ov = pid.clone();
    let fname_ov = filename.clone();
    let rid_ov = rule_id.clone();
    let fname_mode = filename.clone();
    let rid_mode = rule_id.clone();
    let pid_del = pid.clone();
    let fname_del = filename.clone();
    let rid_del = rule_id.clone();

    let presets: &[(&str, &str)] = &[
        ("Hourly", "0 * * * *"),
        ("Daily 02:00", "0 2 * * *"),
        ("Weekdays 09:00", "0 9 * * 1-5"),
        ("Weekly Mon 09:00", "0 9 * * 1"),
    ];
    let tz_presets: &[(&str, &str)] = &[("Local", ""), ("UTC", "UTC")];

    let first_stage = stages
        .first()
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| "backlog".into());

    panel()
        .gap_3()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(label("Schedule Configuration").font_weight(FontWeight::SEMIBOLD))
                .child(compact_action(
                    "Remove schedule rule",
                    Variant::Quiet,
                    move |_| {
                        let mut empty_doc = false;
                        mutate_workflow_file(ui, &pid_del, &fname_del, |doc| {
                            doc.on.remove(&rid_del);
                            empty_doc = doc.on.is_empty();
                        });
                        if empty_doc {
                            ui.workflows.drafts.update(|d| {
                                if let Some(dr) = d.get_mut(&pid_del)
                                    && dr.sources.len() > 1
                                {
                                    dr.sources.remove(&fname_del);
                                    if dr.selected == fname_del {
                                        dr.selected =
                                            dr.sources.keys().next().cloned().unwrap_or_default();
                                    }
                                }
                            });
                        }
                    },
                )),
        )
        .child(
            view()
                .gap_1p5()
                .child(muted("Workflow Name"))
                .child(bound_field(
                    move || wf_name_val.clone(),
                    move |v| {
                        let next = v.to_owned();
                        mutate_workflow_file(ui, &pid_name, &fname_name, move |doc| {
                            doc.name = next;
                        });
                    },
                    "Scheduled Workflow Name…",
                )),
        )
        .child(
            view()
                .gap_1p5()
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(muted("Cron Expression (minute hour day month weekday)"))
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .gap_1()
                                .children(presets.iter().map(|(lbl, expr)| {
                                    let expr_str = (*expr).to_owned();
                                    let p = pid_cron.clone();
                                    let f = fname_cron.clone();
                                    let r = rid_cron.clone();
                                    compact_action(*lbl, Variant::Secondary, move |_| {
                                        let e = expr_str.clone();
                                        let r_inner = r.clone();
                                        mutate_workflow_file(ui, &p, &f, move |doc| {
                                            if let Some(rule) = doc.on.get_mut(&r_inner) {
                                                rule.cron = Some(e);
                                            }
                                        });
                                    })
                                })),
                        ),
                )
                .child({
                    let p = pid_cron.clone();
                    let f = fname_cron.clone();
                    let r = rid_cron.clone();
                    let cval = cur_cron.clone();
                    bound_field(
                        move || cval.clone(),
                        move |v| {
                            let next = v.to_owned();
                            let r_inner = r.clone();
                            mutate_workflow_file(ui, &p, &f, move |doc| {
                                if let Some(rule) = doc.on.get_mut(&r_inner) {
                                    rule.cron = Some(next);
                                }
                            });
                        },
                        "e.g. 0 2 * * * or 0 9 * * 1-5",
                    )
                })
                .child(if let Some(err) = cron_error {
                    notice(err, Tone::Danger).into_element()
                } else {
                    muted(if let Some(next_ts) = next_preview {
                        format!(
                            "{cron_desc} · Next scheduled tick: {next_ts} ({})",
                            item.timezone_label()
                        )
                    } else {
                        cron_desc
                    })
                    .into_element()
                }),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap_4()
                .child(
                    view()
                        .flex_1()
                        .min_w(px(240.))
                        .gap_1p5()
                        .child(muted("Timezone"))
                        .child(view().flex_row().flex_wrap().gap_1().children(
                            tz_presets.iter().map(|(lbl, tz_val)| {
                                let val = (*tz_val).to_owned();
                                let selected_tz = cur_tz == val
                                    || (val.is_empty() && cur_tz.eq_ignore_ascii_case("local"));
                                let p = pid_tz.clone();
                                let f = fname_tz.clone();
                                let r = rid_tz.clone();
                                compact_action(
                                    *lbl,
                                    if selected_tz {
                                        Variant::Primary
                                    } else {
                                        Variant::Secondary
                                    },
                                    move |_| {
                                        let next = val.clone();
                                        let r_inner = r.clone();
                                        mutate_workflow_file(ui, &p, &f, move |doc| {
                                            if let Some(rule) = doc.on.get_mut(&r_inner) {
                                                rule.timezone = (!next.is_empty()).then_some(next);
                                            }
                                        });
                                    },
                                )
                            }),
                        ))
                        .child({
                            let p = pid_tz.clone();
                            let f = fname_tz.clone();
                            let r = rid_tz.clone();
                            let tz_val = cur_tz.clone();
                            bound_field(
                                move || tz_val.clone(),
                                move |v| {
                                    let trimmed = v.trim().to_owned();
                                    let r_inner = r.clone();
                                    mutate_workflow_file(ui, &p, &f, move |doc| {
                                        if let Some(rule) = doc.on.get_mut(&r_inner) {
                                            rule.timezone =
                                                (!trimmed.is_empty()).then_some(trimmed);
                                        }
                                    });
                                },
                                "Local (default), UTC, +08:00, Asia/Shanghai…",
                            )
                        }),
                )
                .child(
                    view()
                        .min_w(px(240.))
                        .gap_1p5()
                        .child(muted("Overlap Policy"))
                        .child(
                            segmented()
                                .child({
                                    let p = pid_ov.clone();
                                    let f = fname_ov.clone();
                                    let r = rid_ov.clone();
                                    compact_segment(
                                        "Skip",
                                        None,
                                        move || cur_overlap == ScheduleOverlap::Skip,
                                        move |_| {
                                            mutate_workflow_file(ui, &p, &f, |doc| {
                                                if let Some(rule) = doc.on.get_mut(&r) {
                                                    rule.overlap = ScheduleOverlap::Skip;
                                                }
                                            });
                                        },
                                    )
                                })
                                .child({
                                    let p = pid_ov.clone();
                                    let f = fname_ov.clone();
                                    let r = rid_ov.clone();
                                    compact_segment(
                                        "Queue 1",
                                        None,
                                        move || cur_overlap == ScheduleOverlap::Queue,
                                        move |_| {
                                            mutate_workflow_file(ui, &p, &f, |doc| {
                                                if let Some(rule) = doc.on.get_mut(&r) {
                                                    rule.overlap = ScheduleOverlap::Queue;
                                                }
                                            });
                                        },
                                    )
                                })
                                .child({
                                    let p = pid_ov.clone();
                                    let f = fname_ov.clone();
                                    let r = rid_ov.clone();
                                    compact_segment(
                                        "Allow",
                                        None,
                                        move || cur_overlap == ScheduleOverlap::Allow,
                                        move |_| {
                                            mutate_workflow_file(ui, &p, &f, |doc| {
                                                if let Some(rule) = doc.on.get_mut(&r) {
                                                    rule.overlap = ScheduleOverlap::Allow;
                                                }
                                            });
                                        },
                                    )
                                }),
                        )
                        .child(muted(match cur_overlap {
                            ScheduleOverlap::Skip => {
                                "Skips the tick if a previous run of this schedule is still active."
                            }
                            ScheduleOverlap::Queue => {
                                "Queues at most 1 pending run until the active run finishes."
                            }
                            ScheduleOverlap::Allow => {
                                "Starts a new run even while an earlier run is still active."
                            }
                        })),
                ),
        )
        .child(execution_mode_section(
            ui,
            item,
            stages,
            creates_card,
            fname_mode,
            rid_mode,
            first_stage,
        ))
        .into_element()
}

fn execution_mode_section(
    ui: Ui,
    item: &ScheduleItem,
    stages: &[(String, String)],
    creates_card: bool,
    fname_mode: String,
    rid_mode: String,
    first_stage: String,
) -> impl IntoElement {
    let pid_agent = item.project_id.clone();
    let fname_agent = fname_mode.clone();
    let rid_agent = rid_mode.clone();
    let pid_card = item.project_id.clone();
    let fname_card = fname_mode.clone();
    let rid_card = rid_mode.clone();
    let is_global = item.is_global();
    let wf_title_default = format!("{} · {{{{date}}}}", item.workflow_name);
    let default_prof = crate::sessions::default_profile(ui);

    view()
        .gap_2p5()
        .pt_2()
        .border_t_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(label("Execution Mode").font_weight(FontWeight::SEMIBOLD))
                .child(
                    segmented()
                        .child(segment(
                            "Cardless (Script / Agent)",
                            Some(Icon::Terminal),
                            move || !creates_card,
                            move |_| {
                                let dprof = default_prof.clone();
                                let rid_inner = rid_agent.clone();
                                mutate_workflow_file(ui, &pid_agent, &fname_agent, move |doc| {
                                    let entry_agent = doc
                                        .steps
                                        .iter()
                                        .find(|s| s.kind() == "agent" || s.kind() == "script")
                                        .map(|s| s.id.clone())
                                        .unwrap_or_else(|| {
                                            let sid = "agent:run".to_owned();
                                            let mut with = BTreeMap::new();
                                            if !dprof.is_empty() {
                                                with.insert("profile".into(), json!(dprof));
                                            }
                                            with.insert(
                                                "instruction".into(),
                                                json!(
                                                    "Run the scheduled check and report findings."
                                                ),
                                            );
                                            doc.steps.insert(
                                                0,
                                                Step {
                                                    id: sid.clone(),
                                                    name: Some("Scheduled Agent".into()),
                                                    uses: "agent".into(),
                                                    with,
                                                    next: Some(
                                                        doc.steps
                                                            .iter()
                                                            .find(|s| s.uses == "complete")
                                                            .map(|s| s.id.clone())
                                                            .unwrap_or_default(),
                                                    )
                                                    .filter(|s| !s.is_empty()),
                                                    ..Default::default()
                                                },
                                            );
                                            sid
                                        });
                                    if let Some(rule) = doc.on.get_mut(&rid_inner) {
                                        rule.card = None;
                                        let cur_start = rule.start_at.trim().to_owned();
                                        let valid_cardless = doc
                                            .steps
                                            .iter()
                                            .find(|s| s.id == cur_start)
                                            .is_some_and(|s| {
                                                matches!(s.kind(), "agent" | "script")
                                            });
                                        if !valid_cardless {
                                            rule.start_at = entry_agent;
                                        }
                                    }
                                });
                            },
                        ))
                        .child(when(
                            move || !is_global,
                            move || {
                                let pid_c = pid_card.clone();
                                let fname_c = fname_card.clone();
                                let col = first_stage.clone();
                                let tpl = wf_title_default.clone();
                                let rid_inner = rid_card.clone();
                                segment(
                                    "Create Card on each run",
                                    Some(Icon::Board),
                                    move || creates_card,
                                    move |_| {
                                        let c = col.clone();
                                        let t = tpl.clone();
                                        let r = rid_inner.clone();
                                        mutate_workflow_file(ui, &pid_c, &fname_c, move |doc| {
                                            if let Some(rule) = doc.on.get_mut(&r)
                                                && rule.card.is_none()
                                            {
                                                rule.card = Some(ScheduleCardConfig {
                                                    column: Some(c),
                                                    title: Some(t),
                                                    body: None,
                                                });
                                            }
                                        });
                                    },
                                )
                            },
                        )),
                ),
        )
        .child(if let Some(card_cfg) = item.rule.card.clone() {
            card_mode_fields(ui, item, stages, card_cfg).into_element()
        } else {
            cardless_mode_fields(ui, item).into_element()
        })
}

fn cardless_mode_fields(ui: Ui, item: &ScheduleItem) -> impl IntoElement {
    let host_profs: Vec<_> = crate::backend::config_for_host(&item.host_id)
        .profiles
        .into_iter()
        .filter(|p| p.enabled)
        .collect();
    let profiles = if host_profs.is_empty() {
        ui.profiles.get()
    } else {
        host_profs
    };
    let cur_prof = item.profile_id.clone().unwrap_or_default();
    let step_id = item.agent_step_id.clone();
    let instruction = item.agent_instruction.clone();
    let pid = draft_key(&item.project_id, &item.host_id);
    let pid_inst = pid.clone();
    let fname = item.filename.clone();
    let fname_inst = item.filename.clone();
    let step_for_inst = step_id.clone();

    view()
        .gap_2()
        .p_3()
        .rounded(px(RADIUS))
        .bg(INSET)
        .child(muted(if item.starts_with_script {
            "Starts directly with a Script step (0 PTY / 0 Agent slot overhead when exiting 0). Escalates to the configured Agent step on non-zero failure, keeping its PTY open in Needs you on `--state blocked`."
        } else {
            "Launches a managed Terminal Agent without creating a Board Card. Once the Agent calls `report --state completed` and settles, its terminal reclaims cleanly."
        }))
        .child(
            view()
                .gap_1()
                .child(muted("Agent Profile"))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_1p5()
                        .children(profiles.into_iter().map(|prof| {
                            let prof_id = prof.id.clone();
                            let selected = prof_id == cur_prof;
                            let scope = pid.clone();
                            let f = fname.clone();
                            let st = step_id.clone();
                            compact_action(
                                format!("@{}", prof.id),
                                if selected {
                                    Variant::Primary
                                } else {
                                    Variant::Secondary
                                },
                                move |_| {
                                    let pval = prof_id.clone();
                                    let target_step = st.clone();
                                    mutate_workflow_file(ui, &scope, &f, move |doc| {
                                        if let Some(sid) = target_step
                                            && let Some(step) =
                                                doc.steps.iter_mut().find(|s| s.id == sid)
                                        {
                                            step.with.insert("profile".into(), json!(pval));
                                        }
                                    });
                                },
                            )
                        })),
                ),
        )
        .child(if let Some(sid) = step_for_inst {
            view()
                .gap_1()
                .child(muted(format!("Task Instruction ({sid})")))
                .child(bound_field(
                    move || instruction.clone(),
                    move |v| {
                        let next = v.to_owned();
                        let target = sid.clone();
                        mutate_workflow_file(ui, &pid_inst, &fname_inst, move |doc| {
                            if let Some(step) = doc.steps.iter_mut().find(|s| s.id == target) {
                                step.with.insert("instruction".into(), json!(next));
                            }
                        });
                    },
                    "Describe what the scheduled Terminal Agent should check and report…",
                ))
                .into_element()
        } else {
            muted("Script-only workflow; edit step commands in Edit YAML.")
                .into_element()
        })
}

fn card_mode_fields(
    ui: Ui,
    item: &ScheduleItem,
    stages: &[(String, String)],
    card_cfg: ScheduleCardConfig,
) -> impl IntoElement {
    let pid = item.project_id.clone();
    let pid_title = item.project_id.clone();
    let pid_body = item.project_id.clone();
    let fname = item.filename.clone();
    let rid = item.rule_id.clone();
    let cur_col = card_cfg
        .column
        .clone()
        .filter(|c| !c.trim().is_empty())
        .or_else(|| stages.first().map(|(id, _)| id.clone()))
        .unwrap_or_else(|| "backlog".into());
    let cur_title = card_cfg.title.unwrap_or_default();
    let cur_body = card_cfg.body.unwrap_or_default();

    let fname_title = fname.clone();
    let rid_title = rid.clone();
    let fname_body = fname.clone();
    let rid_body = rid.clone();

    view()
        .gap_2p5()
        .p_3()
        .rounded(px(RADIUS))
        .bg(INSET)
        .child(muted(
            "Creates a Card with a Scheduled Task badge on every trigger and runs the workflow steps so logs, reports, and diffs attach directly to that Card's Timeline.",
        ))
        .child(
            view()
                .gap_1()
                .child(muted("Target Stage"))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_1p5()
                        .children(stages.iter().map(|(col_id, col_name)| {
                            let cid = col_id.clone();
                            let selected = cid == cur_col;
                            let scope = pid.clone();
                            let f = fname.clone();
                            let r = rid.clone();
                            compact_action(
                                col_name.clone(),
                                if selected {
                                    Variant::Primary
                                } else {
                                    Variant::Secondary
                                },
                                move |_| {
                                    let next_col = cid.clone();
                                    let r_inner = r.clone();
                                    mutate_workflow_file(ui, &scope, &f, move |doc| {
                                        let stage_entry = doc
                                            .column_entry(&next_col)
                                            .map(|(_, step)| step);
                                        if let Some(rule) = doc.on.get_mut(&r_inner) {
                                            let cfg = rule.card.get_or_insert_with(Default::default);
                                            cfg.column = Some(next_col);
                                            if let Some(entry_step) = stage_entry {
                                                rule.start_at = entry_step;
                                            }
                                        }
                                    });
                                },
                            )
                        })),
                ),
        )
        .child(
            view()
                .gap_1()
                .child(muted(
                    "Card Title Template (supports {{date}}, {{datetime}}, {{run_number}}, {{workflow}})",
                ))
                .child(bound_field(
                    move || cur_title.clone(),
                    move |v| {
                        let next = v.to_owned();
                        let r_inner = rid_title.clone();
                        mutate_workflow_file(ui, &pid_title, &fname_title, move |doc| {
                            if let Some(rule) = doc.on.get_mut(&r_inner) {
                                let cfg = rule.card.get_or_insert_with(Default::default);
                                cfg.title = (!next.trim().is_empty()).then_some(next);
                            }
                        });
                    },
                    "e.g. Nightly QA · {{date}} (#{{run_number}})",
                )),
        )
        .child(
            view()
                .gap_1()
                .child(muted("Card Description Template (optional)"))
                .child(bound_field(
                    move || cur_body.clone(),
                    move |v| {
                        let next = v.to_owned();
                        let r_inner = rid_body.clone();
                        mutate_workflow_file(ui, &pid_body, &fname_body, move |doc| {
                            if let Some(rule) = doc.on.get_mut(&r_inner) {
                                let cfg = rule.card.get_or_insert_with(Default::default);
                                cfg.body = (!next.trim().is_empty()).then_some(next);
                            }
                        });
                    },
                    "Optional Markdown instructions or context for each generated Card…",
                )),
        )
}

fn history_panel_section(
    ui: Ui,
    item: &ScheduleItem,
    runs: Memo<Vec<ScheduledRun>>,
) -> impl IntoElement {
    let tz_raw = item.rule.timezone.clone();
    panel()
        .gap_2p5()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    label(move || format!("Recent Runs ({})", runs.with(Vec::len)))
                        .font_weight(FontWeight::SEMIBOLD),
                ),
        )
        .child(when(
            move || runs.with(Vec::is_empty),
            move || {
                muted(
                    "No runs recorded yet. Click Run now to test this scheduled workflow immediately.",
                )
            },
        ))
        .child(when(
            move || !runs.with(Vec::is_empty),
            move || {
                let tz_for_each = tz_raw.clone();
                view().gap_2().child(each(
                    move || runs.get(),
                    |r| format!("{}:{}:{}", r.id, r.state, r.updated_at),
                    move |run_sig, _| {
                        let run = run_sig.get_untracked();
                        let tz_str = tz_for_each.as_deref();
                        let ts = format_run_ts(run.scheduled_at, tz_str);
                        let tone = run_tone(&run.state);
                        let lbl = run_label(&run.state);
                        let live = run.active();
                        let summary_line = run
                            .summary
                            .clone()
                            .or_else(|| run.error.clone())
                            .unwrap_or_default();
                        let card_link = run.card_id.clone().zip(run.card_number);
                        let live_tid = run.terminal_id.clone().and_then(|tid| {
                            ui.snapshot.with_untracked(|s| {
                                let exists = s
                                    .terminals
                                    .iter()
                                    .any(|t| t.id == tid && t.runtime.as_ref().is_some_and(|rt| rt.state != "finished"));
                                (exists || run.active()).then_some(tid)
                            })
                        });
                        let has_report = run.report_path.is_some()
                            || (run.card_id.is_none() && run.summary.is_some());
                        let has_log = run.output_path.is_some();
                        let run_for_report = run.clone();
                        let run_for_log = run.clone();

                        view()
                            .p_2p5()
                            .gap_1p5()
                            .rounded(px(RADIUS))
                            .bg(INSET)
                            .border_1()
                            .border_color(BORDER)
                            .child(
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        view()
                                            .flex_row()
                                            .flex_wrap()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                label(format!("#{}", run.number))
                                                    .font_weight(FontWeight::SEMIBOLD),
                                            )
                                            .child(if live {
                                                live_status(lbl, tone)
                                            } else {
                                                status(lbl, tone)
                                            })
                                            .child(badge(if run.trigger_kind == "manual" {
                                                "Manual"
                                            } else {
                                                "Cron"
                                            }))
                                            .child(if let Some(prof) = &run.profile_id {
                                                badge(format!("@{prof}")).into_element()
                                            } else {
                                                view().into_element()
                                            })
                                            .child(muted(ts)),
                                    )
                                    .child(
                                        view()
                                            .flex_row()
                                            .flex_wrap()
                                            .items_center()
                                            .gap_1p5()
                                            .child(if let Some((cid, cnum)) = card_link {
                                                compact_action(
                                                    format!("Card #{cnum} →"),
                                                    Variant::Secondary,
                                                    move |_| ui.open_card(cid.clone()),
                                                )
                                                .into_element()
                                            } else {
                                                view().into_element()
                                            })
                                            .child(if let Some(tid) = live_tid {
                                                compact_action(
                                                    "Terminal →",
                                                    Variant::Secondary,
                                                    move |_| ui.open_terminal(tid.clone()),
                                                )
                                                .into_element()
                                            } else {
                                                view().into_element()
                                            })
                                            .child(if has_report {
                                                compact_action(
                                                    "View Report",
                                                    Variant::Quiet,
                                                    move |_| {
                                                        let path =
                                                            run_for_report.report_path.as_deref();
                                                        let title = format!(
                                                            "{} #{} · Report",
                                                            run_for_report.workflow_name,
                                                            run_for_report.number
                                                        );
                                                        open_run_artifact(
                                                            ui,
                                                            &run_for_report,
                                                            path,
                                                            title,
                                                        );
                                                    },
                                                )
                                                .into_element()
                                            } else {
                                                view().into_element()
                                            })
                                            .child(if has_log {
                                                compact_action(
                                                    "View Log",
                                                    Variant::Quiet,
                                                    move |_| {
                                                        let path =
                                                            run_for_log.output_path.as_deref();
                                                        let title = format!(
                                                            "{} #{} · Output Log",
                                                            run_for_log.workflow_name,
                                                            run_for_log.number
                                                        );
                                                        open_run_artifact(
                                                            ui,
                                                            &run_for_log,
                                                            path,
                                                            title,
                                                        );
                                                    },
                                                )
                                                .into_element()
                                            } else {
                                                view().into_element()
                                            }),
                                    ),
                            )
                            .child(if !run.steps.is_empty() {
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_1p5()
                                    .children(run.steps.iter().enumerate().map(|(idx, st)| {
                                        let prefix = if idx > 0 { "→ " } else { "" };
                                        let kind_lbl = if let Some(prof) = &st.profile_id {
                                            format!("@{prof}")
                                        } else {
                                            st.step_id.clone()
                                        };
                                        let state_mark = match st.state.as_str() {
                                            "completed" => "✓",
                                            "failed" => "✗",
                                            "blocked" => "⏸",
                                            "running" => "●",
                                            _ => "•",
                                        };
                                        let extra = match (st.exit_code, st.duration_ms) {
                                            (Some(code), Some(ms)) => format!(" ({code} · {ms}ms)"),
                                            (Some(code), None) => format!(" (exit {code})"),
                                            (None, Some(ms)) => format!(" ({ms}ms)"),
                                            (None, None) => String::new(),
                                        };
                                        badge(format!("{prefix}{state_mark} {kind_lbl}{extra}"))
                                    }))
                                    .into_element()
                            } else {
                                view().into_element()
                            })
                            .child(if !summary_line.is_empty() {
                                muted(summary_line).into_element()
                            } else {
                                view().into_element()
                            })
                    },
                ))
            },
        ))
}

fn artifact_dialog(ui: Ui) -> impl IntoElement {
    let s = ui.schedules;
    let editor = component_ref::<nsh_base::Editor>();
    spawn(async move {
        next_frame().await;
        if s.artifact.with_untracked(Option::is_some)
            && let Some(api) = editor.api::<nsh_base::EditorApi>()
        {
            api.focus();
        }
    });
    crate::workflows::workflow_dialog(
        view()
            .flex_1()
            .min_h(px(0.))
            .gap_3()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(icon_tinted(Icon::Clock, ACCENT))
                    .child(heading(move || {
                        s.artifact
                            .with(|a| a.as_ref().map(|(t, _)| t.clone()).unwrap_or_default())
                    }))
                    .child(view().flex_1())
                    .child(chrome_button(
                        Icon::Close,
                        "Close scheduled run report",
                        move |_| s.artifact.set(None),
                    )),
            )
            .child(
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .bg(INSET)
                    .rounded(px(RADIUS))
                    .p_2()
                    .child(
                        nsh_base::editor()
                            .value(move || {
                                Some(
                                    s.artifact
                                        .with(|a| {
                                            a.as_ref().map(|(_, b)| b.clone()).unwrap_or_default()
                                        })
                                        .into(),
                                )
                            })
                            .language(Some("markdown".into()))
                            .read_only(true)
                            .soft_wrap(true)
                            .line_numbers(false)
                            .folding(false)
                            .component_ref(&editor)
                            .accessibility_label("Scheduled run report")
                            .size_full(),
                    ),
            )
            .child(view().flex_row().justify_end().child(action(
                "Close",
                Variant::Secondary,
                move |_| s.artifact.set(None),
            ))),
        move || s.artifact.set(None),
    )
}

pub(crate) fn push_offline_workflow_source(
    s: &mut Snapshot,
    project_id: &str,
    filename: &str,
    yaml: &str,
) -> Result<(String, u64), String> {
    let doc = source::parse(yaml).map_err(|e| e.to_string())?;
    let def = doc.compile().map_err(|e| format!("{e:#}"))?;
    let wf_id = def.id.clone();
    let rev = s
        .workflow_sources
        .iter()
        .find(|ws| ws.project_id == project_id && ws.filename == filename)
        .map_or(1, |ws| ws.revision + 1);
    let rev_id = format!("{wf_id}:{rev}");
    s.workflow_sources
        .retain(|ws| !(ws.project_id == project_id && ws.filename == filename));
    s.workflow_sources.push(WorkflowSource {
        host_id: String::new(),
        id: format!("{project_id}:{filename}"),
        project_id: project_id.to_owned(),
        filename: filename.to_owned(),
        workflow_id: wf_id.clone(),
        revision: rev,
        hash: rev_id.clone(),
        revision_id: rev_id.clone(),
        enabled: doc.enabled,
        columns: def.columns.clone(),
        rules: doc.on.clone(),
    });
    s.workflow_revisions.push(WorkflowRevision {
        host_id: String::new(),
        source_yaml: Some(yaml.to_owned()),
        id: rev_id,
        project_id: project_id.to_owned(),
        workflow_id: wf_id.clone(),
        revision: rev,
        definition: def,
        created_at: canban_types::now(),
    });
    Ok((wf_id, rev))
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Project, Snapshot, Workflow};

    fn fixture_ui() -> Ui {
        let ui = Ui::new();
        let pid = "proj-sched".to_owned();
        let mut snap = Snapshot::default();
        snap.projects.push(Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Vec::new(),
            id: pid.clone(),
            name: "Scheduled Demo".into(),
            directory: "/tmp/proj-sched".into(),
            workflow: Workflow::default(),
            target_branch: Some("main".into()),
            created_at: 1,
        });
        ui.snapshot.set(snap);
        ui.project.set(pid);
        ui.page.set("schedules".into());
        ui
    }

    #[test]
    fn multi_host_schedules_collect_separate_local_and_remote_globals_and_filter_scopes() {
        use canban_types::HostSummary;
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let ui = fixture_ui();
        let mut local = ui.snapshot.get_untracked();
        let now_ms = canban_types::now();
        local.schedule_states.push(ScheduleState {
            host_id: "local".into(),
            id: format!("schedule:{GLOBAL_PROJECT_ID}:global:worktree-gc:schedule"),
            project_id: GLOBAL_PROJECT_ID.into(),
            workflow_id: "global:worktree-gc".into(),
            workflow_name: "Worktree GC".into(),
            filename: "worktree-gc.yaml".into(),
            rule_id: "schedule".into(),
            cron: "0 3 * * *".into(),
            timezone: None,
            overlap: ScheduleOverlap::Skip,
            creates_card: false,
            starts_with_script: true,
            target_column: None,
            profile_id: None,
            enabled: true,
            last_scheduled_at: None,
            next_run_at: Some(now_ms + 3_600_000),
            last_run_id: None,
            run_count: 0,
            queued_triggers: 0,
            updated_at: now_ms,
        });
        local.register_host_entities("local");
        local.host_summaries.insert(
            "local".into(),
            HostSummary {
                id: "local".into(),
                name: crate::platform::LOCAL_HOST_NAME.into(),
                is_local: true,
                transport_label: "Local socket".into(),
                endpoint_label: "~/.canban/app.sock".into(),
                ssh_target: None,
                server_id: None,
                server_pubkey_fingerprint: None,
                connection: canban_types::HostConnectionState::Connected {
                    rtt_ms: 0,
                    api_version: 1,
                    version: "1".into(),
                    platform: "macos".into(),
                },
                active_agents: 0,
                max_agents: 4,
                project_count: 1,
                reconnect_count: 0,
                last_synced_at: None,
                pending_mutations: 0,
                sync_conflicts: 0,
            },
        );

        let mut remote = Snapshot::default();
        remote.projects.push(Project {
            id: "rem-p".into(),
            name: "Remote Repo".into(),
            directory: "/home/devpc/repo".into(),
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: BTreeMap::new(),
            tags: Vec::new(),
            workflow: Workflow::default(),
            target_branch: Some("main".into()),
            created_at: 2,
        });
        remote.schedule_states.push(ScheduleState {
            host_id: String::new(),
            id: format!("schedule:{GLOBAL_PROJECT_ID}:global:worktree-gc:schedule"),
            project_id: GLOBAL_PROJECT_ID.into(),
            workflow_id: "global:worktree-gc".into(),
            workflow_name: "Worktree GC".into(),
            filename: "worktree-gc.yaml".into(),
            rule_id: "schedule".into(),
            cron: "0 4 * * *".into(),
            timezone: None,
            overlap: ScheduleOverlap::Skip,
            creates_card: false,
            starts_with_script: true,
            target_column: None,
            profile_id: None,
            enabled: false,
            last_scheduled_at: None,
            next_run_at: Some(now_ms + 7_200_000),
            last_run_id: None,
            run_count: 0,
            queued_triggers: 0,
            updated_at: now_ms,
        });
        remote.schedule_states.push(ScheduleState {
            host_id: String::new(),
            id: "schedule:rem-p:nightly:schedule".into(),
            project_id: "rem-p".into(),
            workflow_id: "nightly".into(),
            workflow_name: "Remote Nightly".into(),
            filename: "nightly.yaml".into(),
            rule_id: "schedule".into(),
            cron: "30 2 * * *".into(),
            timezone: None,
            overlap: ScheduleOverlap::Skip,
            creates_card: true,
            starts_with_script: false,
            target_column: Some("develop".into()),
            profile_id: Some("developer".into()),
            enabled: true,
            last_scheduled_at: None,
            next_run_at: Some(now_ms + 5_000_000),
            last_run_id: None,
            run_count: 0,
            queued_triggers: 0,
            updated_at: now_ms,
        });
        local.merge_remote_snapshot(
            remote,
            HostSummary {
                id: "devpc".into(),
                name: "devpc".into(),
                is_local: false,
                transport_label: "SSH".into(),
                endpoint_label: "devpc".into(),
                ssh_target: Some("devpc".into()),
                server_id: None,
                server_pubkey_fingerprint: None,
                connection: canban_types::HostConnectionState::Connected {
                    rtt_ms: 8,
                    api_version: 1,
                    version: "1".into(),
                    platform: "linux".into(),
                },
                active_agents: 0,
                max_agents: 4,
                project_count: 1,
                reconnect_count: 0,
                last_synced_at: None,
                pending_mutations: 0,
                sync_conflicts: 0,
            },
        );
        ui.snapshot.set(local);

        let all = collect_schedules(ui, "");
        assert_eq!(all.len(), 3);
        let keys: std::collections::BTreeSet<_> = all.iter().map(|i| i.key.clone()).collect();
        assert_eq!(keys.len(), 3);
        assert!(all.iter().any(|i| i.host_id == "local"
            && i.scope_name == format!("Global · {}", crate::platform::LOCAL_HOST_NAME)));
        assert!(
            all.iter()
                .any(|i| i.host_id == "devpc" && i.scope_name == "Global · devpc")
        );
        assert!(
            all.iter()
                .any(|i| i.host_id == "devpc" && i.workflow_name == "Remote Nightly")
        );

        let devpc_globals = collect_schedules(ui, &encode_scope_key(GLOBAL_PROJECT_ID, "devpc"));
        assert_eq!(devpc_globals.len(), 1);
        assert_eq!(devpc_globals[0].host_id, "devpc");
        assert_eq!(devpc_globals[0].rule.cron.as_deref(), Some("0 4 * * *"));
    }
}
