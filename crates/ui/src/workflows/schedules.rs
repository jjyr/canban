//! Scheduled Tasks panel inside `Project / Workflows`: view, configure, toggle,
//! trigger (`Run now`), and inspect execution history for cron-scheduled workflows.

use super::{Draft, source_dirty, workflow_dialog};
use crate::presenter::*;
use canban_types::{
    Card, ScheduleCardConfig, ScheduleOverlap, ScheduleSourceRef, ScheduleState, ScheduledRun,
    WorkflowRule,
    cron::{CronTimeZone, describe_cron, parse_schedule},
};
use canban_ui_kit::{tokens::*, *};
use canban_workflow_engine::source::{self, Document, Step};
use nsh_core::prelude::*;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduleItem {
    /// Canonical UI selection key: `<filename>::<rule_id>`.
    pub key: String,
    pub schedule_id: String,
    pub project_id: String,
    pub filename: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub workflow_enabled: bool,
    pub rule_id: String,
    pub rule: WorkflowRule,
    pub effective_enabled: bool,
    pub dirty: bool,
    pub published: bool,
    pub profile_id: Option<String>,
    pub agent_step_id: Option<String>,
    pub agent_instruction: String,
    pub next_run_at: Option<u64>,
    pub last_scheduled_at: Option<u64>,
    pub run_count: u64,
    pub queued_triggers: u32,
}

impl ScheduleItem {
    pub fn cron_str(&self) -> &str {
        self.rule.cron.as_deref().unwrap_or("").trim()
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
        } else if let Some(prof) = &self.profile_id {
            format!("Terminal Agent · @{prof}")
        } else {
            "Terminal Agent".into()
        }
    }
}

pub fn schedule_key(filename: &str, rule_id: &str) -> String {
    format!("{filename}::{rule_id}")
}

fn find_agent_step_for_rule(
    doc: &Document,
    rule: &WorkflowRule,
) -> Option<(String, Option<String>, String)> {
    let start = if rule.start_at.trim().is_empty() {
        doc.steps.first().map(|s| s.id.as_str())?
    } else {
        rule.start_at.trim()
    };
    let mut cur = Some(start.to_owned());
    let mut seen = std::collections::BTreeSet::new();
    while let Some(step_id) = cur {
        if !seen.insert(step_id.clone()) {
            break;
        }
        let idx = doc.steps.iter().position(|s| s.id == step_id)?;
        let step = &doc.steps[idx];
        if step.kind() == "agent" {
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
            return Some((step.id.clone(), profile, instruction));
        }
        cur = step
            .next
            .clone()
            .or_else(|| doc.steps.get(idx + 1).map(|s| s.id.clone()));
    }
    None
}

fn project_stages(ui: Ui, project_id: &str) -> Vec<(String, String)> {
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

/// Collects all scheduled rules across the Project's current workflow draft (or saved snapshot).
pub fn collect_project_schedules(ui: Ui) -> Vec<ScheduleItem> {
    let project_id = ui.project.get();
    if project_id.is_empty() {
        return Vec::new();
    }
    let state = ui.workflows;
    let draft_opt = state.drafts.with(|d| d.get(&project_id).cloned());
    ui.snapshot.with(|s| {
        let mut items = Vec::new();
        let now_ms = canban_types::now();
        let sources_map: BTreeMap<String, String> = if let Some(draft) = &draft_opt
            && !draft.sources.is_empty()
        {
            draft.sources.clone()
        } else {
            super::project_template_files_from_snapshot(s, &project_id)
                .into_iter()
                .map(|f| (f.filename, f.source))
                .collect()
        };

        for (filename, raw) in &sources_map {
            let dirty = draft_opt
                .as_ref()
                .is_some_and(|d| source_dirty(d, filename));
            let saved_src = s
                .workflow_sources
                .iter()
                .find(|ws| ws.project_id == project_id && &ws.filename == filename);
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
                let sid = format!("schedule:{project_id}:{wf_id}:{rule_id}");
                let persisted = s.schedule_states.iter().find(|st| {
                    st.id == sid
                        || (st.project_id == project_id
                            && st.filename == *filename
                            && st.rule_id == *rule_id)
                });
                let effective_enabled = doc.enabled && rule.enabled;
                let (agent_step_id, doc_profile, agent_instruction) =
                    match find_agent_step_for_rule(&doc, rule) {
                        Some((id, prof, inst)) => (Some(id), prof, inst),
                        None => (None, None, String::new()),
                    };
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
                items.push(ScheduleItem {
                    key: schedule_key(filename, rule_id),
                    schedule_id: persisted.map(|p| p.id.clone()).unwrap_or(sid),
                    project_id: project_id.clone(),
                    filename: filename.clone(),
                    workflow_id: wf_id.clone(),
                    workflow_name: wf_name.clone(),
                    workflow_enabled: doc.enabled,
                    rule_id: rule_id.clone(),
                    rule: rule.clone(),
                    effective_enabled,
                    dirty,
                    published: persisted.is_some() && !dirty,
                    profile_id: doc_profile
                        .or_else(|| persisted.and_then(|p| p.profile_id.clone())),
                    agent_step_id,
                    agent_instruction,
                    next_run_at,
                    last_scheduled_at: persisted.and_then(|p| p.last_scheduled_at),
                    run_count: persisted.map_or(0, |p| p.run_count),
                    queued_triggers: persisted.map_or(0, |p| p.queued_triggers),
                });
            }
        }
        items
    })
}

pub fn project_schedule_count(ui: Ui) -> usize {
    collect_project_schedules(ui).len()
}

pub fn open_schedules(ui: Ui, select_schedule: Option<String>) {
    ui.workflows.section.set("schedules".into());
    if let Some(sel) = select_schedule {
        ui.workflows.selected_schedule.set(sel);
    }
    if ui.page.get_untracked() != "workflows" {
        ui.navigate("workflows");
    }
}

fn ensure_draft_initialized(ui: Ui, project_id: &str) {
    let state = ui.workflows;
    let has = state.drafts.with_untracked(|d| d.contains_key(project_id));
    if has {
        return;
    }
    let files = ui
        .snapshot
        .with_untracked(|s| super::project_template_files_from_snapshot(s, project_id));
    if files.is_empty() {
        return;
    }
    let base_sources: Vec<_> = files
        .iter()
        .map(|f| json!({"filename": f.filename, "source": f.source}))
        .collect();
    state.drafts.update(|d| {
        d.entry(project_id.to_owned())
            .or_default()
            .accept(json!({"revision": "local", "sources": base_sources}));
    });
}

pub fn new_scheduled_workflow(ui: Ui) {
    let project = ui.project.get_untracked();
    if project.is_empty() {
        ui.notify("Select or create a Project first");
        return;
    }
    ensure_draft_initialized(ui, &project);
    let state = ui.workflows;
    let default_prof = crate::sessions::default_profile(ui);
    let prof_opt = (!default_prof.is_empty()).then_some(default_prof.as_str());
    let mut filename = String::new();
    state.drafts.update(|d| {
        let draft = d.entry(project.clone()).or_default();
        let mut n = 1usize;
        while draft.sources.contains_key(&format!("scheduled-{n}.yaml")) {
            n += 1;
        }
        filename = format!("scheduled-{n}.yaml");
        let doc = source::default_scheduled_document(
            format!("scheduled-{n}:{project}"),
            format!("Scheduled Task {n}"),
            prof_opt,
        );
        if let Ok(yaml) = source::encode(&doc) {
            draft.sources.insert(filename.clone(), yaml);
            draft.selected = filename.clone();
        }
    });
    if !filename.is_empty() {
        state.selected.set(filename.clone());
        state.section.set("schedules".into());
        state
            .selected_schedule
            .set(schedule_key(&filename, "schedule"));
    }
    if ui.page.get_untracked() != "workflows" {
        ui.navigate("workflows");
    }
}

pub fn add_schedule_rule_to_file(ui: Ui, filename: &str) {
    let project = ui.project.get_untracked();
    if project.is_empty() || filename.is_empty() {
        return;
    }
    ensure_draft_initialized(ui, &project);
    let default_prof = crate::sessions::default_profile(ui);
    let mut added_rule_id = String::new();
    mutate_workflow_file(ui, filename, |doc| {
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
        ui.workflows.section.set("schedules".into());
        ui.workflows
            .selected_schedule
            .set(schedule_key(filename, &added_rule_id));
    }
}

pub(super) fn mutate_workflow_file(ui: Ui, filename: &str, mutate: impl FnOnce(&mut Document)) {
    let state = ui.workflows;
    let project = ui.project.get_untracked();
    if project.is_empty() || filename.is_empty() {
        return;
    }
    ensure_draft_initialized(ui, &project);
    let Some(raw) = state.drafts.with_untracked(|d| {
        d.get(&project)
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
            .replace((project.clone(), filename.to_owned(), encoded.clone()));
        state.studio_error.set(String::new());
        state.studio_doc.set(Some(doc));
    }
    state.drafts.update(|d| {
        if let Some(dr) = d.get_mut(&project) {
            dr.sources.insert(filename.to_owned(), encoded);
        }
    });
}

pub fn toggle_schedule(ui: Ui, item: &ScheduleItem, enabled: bool) {
    let state = ui.workflows;
    let project = item.project_id.clone();
    let filename = item.filename.clone();
    let rule_id = item.rule_id.clone();

    let draft_dirty = state
        .drafts
        .with_untracked(|d| d.get(&project).is_some_and(Draft::needs_apply));

    // If there are unsaved draft edits, or this schedule has not been published yet,
    // mutate the draft directly so the user's unsaved edits are never clobbered.
    if draft_dirty || !item.published {
        mutate_workflow_file(ui, &filename, |doc| {
            if enabled {
                doc.enabled = true;
            }
            if let Some(rule) = doc.on.get_mut(&rule_id) {
                rule.enabled = enabled;
            }
        });
        return;
    }

    if crate::APP.get().is_none() {
        mutate_workflow_file(ui, &filename, |doc| {
            if enabled {
                doc.enabled = true;
            }
            if let Some(rule) = doc.on.get_mut(&rule_id) {
                rule.enabled = enabled;
            }
        });
        // Mark draft clean in offline Gallery mode and sync ScheduleState.
        let now_ms = canban_types::now();
        state.drafts.update(|d| {
            if let Some(dr) = d.get_mut(&project) {
                dr.base = json!({
                    "revision": "gallery",
                    "sources": dr.inputs(),
                });
            }
        });
        let sid = item.schedule_id.clone();
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
        ui.notify(if enabled {
            "Schedule enabled"
        } else {
            "Schedule paused"
        });
        return;
    }

    let sid = item.schedule_id.clone();
    dispatch_complete(
        ui,
        "schedule.toggle",
        json!({
            "schedule_id": sid,
            "enabled": enabled,
        }),
        move |res| match res {
            Ok(_) => {
                super::load(ui, true);
                ui.notify(if enabled {
                    "Schedule enabled"
                } else {
                    "Schedule paused"
                });
            }
            Err(e) => {
                state.error.set(e);
                ui.operation_error.set(String::new());
            }
        },
    );
}

pub fn trigger_schedule(ui: Ui, item: &ScheduleItem) {
    let state = ui.workflows;
    let project = item.project_id.clone();
    let has_unsaved = state
        .drafts
        .with_untracked(|d| d.get(&project).is_some_and(Draft::needs_apply));
    if has_unsaved || !item.published {
        ui.notify("Save workflow changes first (⌘S) before running this schedule");
        return;
    }

    if crate::APP.get().is_none() {
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
    dispatch_complete(
        ui,
        "schedule.trigger",
        json!({ "schedule_id": sid }),
        move |res| match res {
            Ok(v) => {
                if let Some(cnum) = v["card_number"].as_u64() {
                    ui.notify(format!("Triggered schedule · Created Card #{cnum}"));
                } else {
                    ui.notify("Scheduled task triggered");
                }
            }
            Err(e) => {
                state.error.set(e);
                ui.operation_error.set(String::new());
            }
        },
    );
}

pub fn trigger_schedule_by_id(ui: Ui, schedule_id: &str) {
    let items = collect_project_schedules(ui);
    if let Some(item) = items
        .iter()
        .find(|i| i.schedule_id == schedule_id || i.key == schedule_id)
    {
        trigger_schedule(ui, item);
        return;
    }
    if crate::APP.get().is_some() {
        dispatch(
            ui,
            "schedule.trigger",
            json!({ "schedule_id": schedule_id }),
        );
    }
}

pub fn toggle_schedule_by_id(ui: Ui, schedule_id: &str) {
    let items = collect_project_schedules(ui);
    if let Some(item) = items
        .iter()
        .find(|i| i.schedule_id == schedule_id || i.key == schedule_id)
    {
        toggle_schedule(ui, item, !item.effective_enabled);
        return;
    }
    let cur = ui.snapshot.with_untracked(|s| {
        s.schedule_states
            .iter()
            .find(|st| st.id == schedule_id)
            .map(|st| st.enabled)
    });
    if let Some(enabled) = cur
        && crate::APP.get().is_some()
    {
        dispatch(
            ui,
            "schedule.toggle",
            json!({ "schedule_id": schedule_id, "enabled": !enabled }),
        );
    }
}

pub(super) fn offline_save_schedules(ui: Ui, project_id: &str) {
    let items = collect_project_schedules(ui);
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

fn format_run_ts(ts: u64, tz_raw: Option<&str>) -> String {
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
        "blocked" => "Blocked",
        "skipped" => "Skipped",
        "failed" => "Failed",
        "cancelled" => "Cancelled",
        "unknown" => "Interrupted",
        _ => "Finished",
    }
}

fn open_run_artifact(ui: Ui, run: &ScheduledRun, path: Option<&str>, title: String) {
    let state = ui.workflows;
    if crate::APP.get().is_none() || path.is_none() {
        let body = run
            .summary
            .clone()
            .or_else(|| run.error.clone())
            .unwrap_or_else(|| "No report text was recorded for this scheduled run.".into());
        state.schedule_artifact.set(Some((title, body)));
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
                state.schedule_artifact.set(Some((title, body)));
            }
            Ok(Err(e)) => {
                if !fallback.is_empty() {
                    state.schedule_artifact.set(Some((title, fallback)));
                } else {
                    state.error.set(format!("{e:#}"));
                }
            }
            Err(e) => state.error.set(e.to_string()),
        }
    });
}

pub fn render(ui: Ui) -> AnyElement {
    let state = ui.workflows;
    let items = computed(move || collect_project_schedules(ui));
    let stages = computed(move || {
        let pid = ui.project.get();
        project_stages(ui, &pid)
    });

    // Keep `selected_schedule` pointed at a valid schedule item when available.
    effect(move || {
        let list = items.get();
        let cur = state.selected_schedule.get_untracked();
        if list.is_empty() {
            return;
        }
        let matches_cur = list.iter().any(|i| i.key == cur || i.schedule_id == cur);
        if !matches_cur {
            state.selected_schedule.set(list[0].key.clone());
        }
    });

    let selected_item = computed(move || {
        let cur = state.selected_schedule.get();
        let list = items.get();
        list.into_iter()
            .find(|i| i.key == cur || i.schedule_id == cur)
    });

    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .child(when(
            move || items.with(Vec::is_empty),
            move || empty_state(ui),
        ))
        .child(when(
            move || !items.with(Vec::is_empty),
            move || {
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .flex_row()
                    .gap_4()
                    .child(master_list(ui, items, stages))
                    .child(dynamic(move || {
                        if let Some(item) = selected_item.get() {
                            detail_pane(ui, item, stages.get())
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
            move || state.schedule_artifact.get().is_some(),
            move || artifact_dialog(ui),
        ))
        .into_element()
}

fn empty_state(ui: Ui) -> impl IntoElement {
    let state = ui.workflows;
    panel()
        .flex_1()
        .min_h(px(0.))
        .items_center()
        .justify_center()
        .gap_3()
        .border_1()
        .border_color(BORDER)
        .child(tone_icon_badge(Icon::Clock, Tone::Info, false))
        .child(heading("No Scheduled Tasks Configured"))
        .child(
            muted(
                "Configure GitHub Actions-style cron triggers in Workflow YAML to launch periodic Terminal Agents or automatically create scheduled Cards.",
            )
            .max_w(px(520.)),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    action_with_icon(
                        Icon::Plus,
                        "New scheduled workflow",
                        Variant::Primary,
                        move |_| new_scheduled_workflow(ui),
                    )
                    .disabled(move || ui.pending.get() || state.loading.get()),
                )
                .child(when(
                    move || !state.selected.get().is_empty(),
                    move || {
                        let sel = state.selected.get();
                        action_with_icon(
                            Icon::Clock,
                            format!("Add schedule to {sel}"),
                            Variant::Secondary,
                            move |_| {
                                let cur = state.selected.get_untracked();
                                add_schedule_rule_to_file(ui, &cur);
                            },
                        )
                    },
                )),
        )
}

fn master_list(
    ui: Ui,
    items: Memo<Vec<ScheduleItem>>,
    stages: Memo<Vec<(String, String)>>,
) -> impl IntoElement {
    let state = ui.workflows;
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
                .child(
                    compact_action("+ New schedule", Variant::Secondary, move |_| {
                        new_scheduled_workflow(ui);
                    })
                    .disabled(move || ui.pending.get() || state.loading.get()),
                ),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .child(view().gap_2().pr_1().child(each(
                    move || items.get(),
                    |item| item.key.clone(),
                    move |item_sig, _| {
                        let key = item_sig.with_untracked(|i| i.key.clone());
                        let sid = item_sig.with_untracked(|i| i.schedule_id.clone());
                        let select_key = key.clone();
                        let card_key = key.clone();
                        let card_sid = sid.clone();
                        let is_selected = computed(move || {
                            let cur = state.selected_schedule.get();
                            cur == card_key || cur == card_sid
                        });
                        press_target()
                            .w_full()
                            .p_3()
                            .gap_2()
                            .rounded(px(RADIUS))
                            .border_1()
                            .border_color(move || if is_selected.get() { ACCENT } else { BORDER })
                            .bg(move || if is_selected.get() { SELECTED } else { SURFACE })
                            .hover(|s| s.border_color(ACCENT))
                            .selected(move || is_selected.get())
                            .accessibility_label(move || {
                                item_sig.with(|it| {
                                    Some(
                                        format!(
                                            "{} · {} · {}",
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
                                let filename = item_sig.with_untracked(|i| i.filename.clone());
                                let project = ui.project.get_untracked();
                                state.drafts.update(|d| {
                                    if let Some(dr) = d.get_mut(&project) {
                                        dr.selected = filename.clone();
                                    }
                                });
                                state.selected.set(filename);
                                state.selected_schedule.set(select_key.clone());
                            })
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .min_w(px(0.))
                                    .child(
                                        icon_tinted(Icon::Clock, move || {
                                            if item_sig.with(|i| i.effective_enabled) {
                                                ACCENT
                                            } else {
                                                MUTED
                                            }
                                        })
                                        .w(px(15.))
                                        .h(px(15.)),
                                    )
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
                                    .child(dynamic(move || {
                                        let enabled = item_sig.with(|i| i.effective_enabled);
                                        let active_run = ui.snapshot.with(|s| {
                                            let sid = item_sig.with(|i| i.schedule_id.clone());
                                            s.scheduled_runs
                                                .iter()
                                                .any(|r| r.schedule_id == sid && r.active())
                                        });
                                        if active_run {
                                            live_status("Running", Tone::Info).into_element()
                                        } else if enabled {
                                            status("Active", Tone::Success).into_element()
                                        } else {
                                            status("Paused", Tone::Neutral).into_element()
                                        }
                                    })),
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
                                    .child(badge(move || {
                                        let stg = stages.get();
                                        item_sig.with(|i| i.mode_badge_text(&stg))
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
                    active_terminal_id,
                ))
                .child(config_panel(
                    ui,
                    &item,
                    &stages,
                    cron_error,
                    cron_desc,
                    next_preview,
                ))
                .child(history_panel_section(ui, &item, runs)),
        )
        .into_element()
}

fn detail_header_card(
    ui: Ui,
    item: &ScheduleItem,
    item_for_toggle: ScheduleItem,
    item_for_run: ScheduleItem,
    active_terminal_id: Option<String>,
) -> impl IntoElement {
    let state = ui.workflows;
    let filename = item.filename.clone();
    let filename_yaml = item.filename.clone();
    let filename_studio = item.filename.clone();
    let enabled = item.effective_enabled;
    let wf_name = item.workflow_name.clone();
    let rule_id = item.rule_id.clone();
    let dirty = item.dirty;
    let published = item.published;
    let queued = item.queued_triggers;
    let run_count = item.run_count;

    panel().gap_2p5().border_1().border_color(BORDER).child(
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
                        if enabled { Tone::Info } else { Tone::Neutral },
                        false,
                    ))
                    .child(
                        view()
                            .gap_0p5()
                            .min_w(px(0.))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(heading(if rule_id == "schedule" {
                                        wf_name.clone()
                                    } else {
                                        format!("{wf_name} / {rule_id}")
                                    }))
                                    .child(badge(filename))
                                    .child(if dirty {
                                        status("Unsaved draft", Tone::Attention)
                                    } else if enabled {
                                        status("Enabled", Tone::Success)
                                    } else {
                                        status("Paused", Tone::Neutral)
                                    }),
                            )
                            .child(muted(format!(
                                "{run_count} total run{}{}",
                                if run_count == 1 { "" } else { "s" },
                                if queued > 0 {
                                    " · 1 queued overlap trigger"
                                } else {
                                    ""
                                }
                            ))),
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
                    .child(compact_action(
                        if enabled { "Pause" } else { "Enable" },
                        Variant::Secondary,
                        move |_| {
                            toggle_schedule(ui, &item_for_toggle, !enabled);
                        },
                    ))
                    .child(
                        action_with_icon(Icon::Zap, "Run now", Variant::Primary, move |_| {
                            trigger_schedule(ui, &item_for_run)
                        })
                        .disabled(move || {
                            ui.is_method_pending("schedule.trigger")
                                || state.loading.get()
                                || dirty
                                || !published
                        }),
                    )
                    .child(compact_action("Open Studio", Variant::Quiet, move |_| {
                        let p = ui.project.get_untracked();
                        state.drafts.update(|d| {
                            if let Some(dr) = d.get_mut(&p) {
                                dr.selected = filename_studio.clone();
                            }
                        });
                        state.selected.set(filename_studio.clone());
                        state.mode.set("visual".into());
                        state.section.set("workflows".into());
                    }))
                    .child(compact_action("YAML", Variant::Quiet, move |_| {
                        let p = ui.project.get_untracked();
                        state.drafts.update(|d| {
                            if let Some(dr) = d.get_mut(&p) {
                                dr.selected = filename_yaml.clone();
                            }
                        });
                        state.selected.set(filename_yaml.clone());
                        state.mode.set("yaml".into());
                        state.section.set("workflows".into());
                    })),
            ),
    )
}

fn config_panel(
    ui: Ui,
    item: &ScheduleItem,
    stages: &[(String, String)],
    cron_error: Option<String>,
    cron_desc: String,
    next_preview: Option<String>,
) -> impl IntoElement {
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

    let fname_name = filename.clone();
    let fname_cron = filename.clone();
    let rid_cron = rule_id.clone();
    let fname_tz = filename.clone();
    let rid_tz = rule_id.clone();
    let fname_ov = filename.clone();
    let rid_ov = rule_id.clone();
    let fname_mode = filename.clone();
    let rid_mode = rule_id.clone();
    let fname_del = filename.clone();
    let rid_del = rule_id.clone();

    let presets: &[(&str, &str)] = &[
        ("Hourly", "0 * * * *"),
        ("Daily 02:00", "0 2 * * *"),
        ("Weekdays 09:00", "0 9 * * 1-5"),
        ("Weekly Mon 09:00", "0 9 * * 1"),
    ];
    let tz_presets: &[(&str, &str)] = &[
        ("Local", ""),
        ("UTC", "UTC"),
        ("Asia/Shanghai", "Asia/Shanghai"),
        ("America/New_York", "America/New_York"),
        ("Europe/London", "Europe/London"),
        ("Asia/Tokyo", "Asia/Tokyo"),
    ];

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
                        mutate_workflow_file(ui, &fname_del, |doc| {
                            doc.on.remove(&rid_del);
                            empty_doc = doc.on.is_empty();
                        });
                        if empty_doc {
                            let p = ui.project.get_untracked();
                            ui.workflows.drafts.update(|d| {
                                if let Some(dr) = d.get_mut(&p)
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
                        mutate_workflow_file(ui, &fname_name, move |doc| {
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
                                    let f = fname_cron.clone();
                                    let r = rid_cron.clone();
                                    compact_action(*lbl, Variant::Secondary, move |_| {
                                        let e = expr_str.clone();
                                        let r_inner = r.clone();
                                        mutate_workflow_file(ui, &f, move |doc| {
                                            if let Some(rule) = doc.on.get_mut(&r_inner) {
                                                rule.cron = Some(e);
                                            }
                                        });
                                    })
                                })),
                        ),
                )
                .child({
                    let f = fname_cron.clone();
                    let r = rid_cron.clone();
                    let cval = cur_cron.clone();
                    bound_field(
                        move || cval.clone(),
                        move |v| {
                            let next = v.to_owned();
                            let r_inner = r.clone();
                            mutate_workflow_file(ui, &f, move |doc| {
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
                                        mutate_workflow_file(ui, &f, move |doc| {
                                            if let Some(rule) = doc.on.get_mut(&r_inner) {
                                                rule.timezone = (!next.is_empty()).then_some(next);
                                            }
                                        });
                                    },
                                )
                            }),
                        ))
                        .child({
                            let f = fname_tz.clone();
                            let r = rid_tz.clone();
                            let tz_val = cur_tz.clone();
                            bound_field(
                                move || tz_val.clone(),
                                move |v| {
                                    let trimmed = v.trim().to_owned();
                                    let r_inner = r.clone();
                                    mutate_workflow_file(ui, &f, move |doc| {
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
                                    let f = fname_ov.clone();
                                    let r = rid_ov.clone();
                                    compact_segment(
                                        "Skip",
                                        None,
                                        move || cur_overlap == ScheduleOverlap::Skip,
                                        move |_| {
                                            mutate_workflow_file(ui, &f, |doc| {
                                                if let Some(rule) = doc.on.get_mut(&r) {
                                                    rule.overlap = ScheduleOverlap::Skip;
                                                }
                                            });
                                        },
                                    )
                                })
                                .child({
                                    let f = fname_ov.clone();
                                    let r = rid_ov.clone();
                                    compact_segment(
                                        "Queue 1",
                                        None,
                                        move || cur_overlap == ScheduleOverlap::Queue,
                                        move |_| {
                                            mutate_workflow_file(ui, &f, |doc| {
                                                if let Some(rule) = doc.on.get_mut(&r) {
                                                    rule.overlap = ScheduleOverlap::Queue;
                                                }
                                            });
                                        },
                                    )
                                })
                                .child({
                                    let f = fname_ov.clone();
                                    let r = rid_ov.clone();
                                    compact_segment(
                                        "Allow",
                                        None,
                                        move || cur_overlap == ScheduleOverlap::Allow,
                                        move |_| {
                                            mutate_workflow_file(ui, &f, |doc| {
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
    let fname_agent = fname_mode.clone();
    let rid_agent = rid_mode.clone();
    let fname_card = fname_mode.clone();
    let rid_card = rid_mode.clone();
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
                            "Terminal Agent only",
                            Some(Icon::Terminal),
                            move || !creates_card,
                            move |_| {
                                let dprof = default_prof.clone();
                                let rid_inner = rid_agent.clone();
                                mutate_workflow_file(ui, &fname_agent, move |doc| {
                                    let entry_agent = doc
                                        .steps
                                        .iter()
                                        .find(|s| s.kind() == "agent")
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
                        .child(segment(
                            "Create Card on each run",
                            Some(Icon::Board),
                            move || creates_card,
                            move |_| {
                                let col = first_stage.clone();
                                let tpl = wf_title_default.clone();
                                let rid_inner = rid_card.clone();
                                mutate_workflow_file(ui, &fname_card, move |doc| {
                                    if let Some(rule) = doc.on.get_mut(&rid_inner)
                                        && rule.card.is_none()
                                    {
                                        rule.card = Some(ScheduleCardConfig {
                                            column: Some(col),
                                            title: Some(tpl),
                                            body: None,
                                        });
                                    }
                                });
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
    let profiles = ui.profiles.get();
    let cur_prof = item.profile_id.clone().unwrap_or_default();
    let step_id = item.agent_step_id.clone();
    let instruction = item.agent_instruction.clone();
    let fname = item.filename.clone();
    let fname_inst = item.filename.clone();
    let step_for_inst = step_id.clone();

    view()
        .gap_2()
        .p_3()
        .rounded(px(RADIUS))
        .bg(INSET)
        .child(muted(
            "Launches a managed Terminal Agent directly in the Project root without creating a Board Card. Once the Agent reports completion and settles, its terminal automatically exits and reclaims cleanly.",
        ))
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
                            let pid = prof.id.clone();
                            let selected = pid == cur_prof;
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
                                    let pval = pid.clone();
                                    let target_step = st.clone();
                                    mutate_workflow_file(ui, &f, move |doc| {
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
                        mutate_workflow_file(ui, &fname_inst, move |doc| {
                            if let Some(step) = doc.steps.iter_mut().find(|s| s.id == target) {
                                step.with.insert("instruction".into(), json!(next));
                            }
                        });
                    },
                    "Describe what the scheduled Terminal Agent should check and report…",
                ))
                .into_element()
        } else {
            muted("Entry step is a script step; edit full step chain in Visual Studio or YAML.")
                .into_element()
        })
}

fn card_mode_fields(
    ui: Ui,
    item: &ScheduleItem,
    stages: &[(String, String)],
    card_cfg: ScheduleCardConfig,
) -> impl IntoElement {
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
                                    mutate_workflow_file(ui, &f, move |doc| {
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
                        mutate_workflow_file(ui, &fname_title, move |doc| {
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
                        mutate_workflow_file(ui, &fname_body, move |doc| {
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
    let state = ui.workflows;
    let editor = component_ref::<nsh_base::Editor>();
    spawn(async move {
        next_frame().await;
        if state.schedule_artifact.with_untracked(Option::is_some)
            && let Some(api) = editor.api::<nsh_base::EditorApi>()
        {
            api.focus();
        }
    });
    workflow_dialog(
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
                        state
                            .schedule_artifact
                            .with(|a| a.as_ref().map(|(t, _)| t.clone()).unwrap_or_default())
                    }))
                    .child(view().flex_1())
                    .child(chrome_button(
                        Icon::Close,
                        "Close scheduled run report",
                        move |_| state.schedule_artifact.set(None),
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
                                    state
                                        .schedule_artifact
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
                move |_| state.schedule_artifact.set(None),
            ))),
        move || state.schedule_artifact.set(None),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Project, Snapshot, Workflow};
    use nsh_core::{ApproxTextEngine, Core, MountItem, geometry::size};

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
            location: None,
            workflow: Workflow::default(),
            target_branch: Some("main".into()),
            created_at: 1,
        });
        ui.snapshot.set(snap);
        ui.project.set(pid);
        ui.page.set("workflows".into());
        ui
    }

    #[test]
    fn offline_new_scheduled_workflow_saves_triggers_and_toggles_both_modes() {
        #[component]
        fn SchedulesProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = fixture_ui();
            assert_eq!(collect_project_schedules(ui).len(), 0);

            // 1. Create a new Cardless Terminal Agent schedule and save offline.
            new_scheduled_workflow(ui);
            assert_eq!(ui.workflows.section.get_untracked(), "schedules");
            let items = collect_project_schedules(ui);
            assert_eq!(items.len(), 1);
            assert!(items[0].dirty);
            assert_eq!(items[0].filename, "scheduled-1.yaml");
            assert!(items[0].rule.card.is_none());

            super::super::save(ui);
            let items = collect_project_schedules(ui);
            assert_eq!(items.len(), 1);
            assert!(!items[0].dirty);
            assert!(items[0].published);
            assert!(items[0].effective_enabled);

            // 2. Trigger Cardless run offline -> creates a ScheduledRun without creating a Card.
            trigger_schedule(ui, &items[0]);
            ui.snapshot.with_untracked(|s| {
                assert_eq!(s.cards.len(), 0);
                assert_eq!(s.scheduled_runs.len(), 1);
                assert_eq!(s.scheduled_runs[0].trigger_kind, "manual");
                assert!(s.scheduled_runs[0].card_id.is_none());
            });

            // 3. Toggle pause / re-enable offline.
            toggle_schedule(ui, &items[0], false);
            let paused = collect_project_schedules(ui);
            assert!(!paused[0].effective_enabled);
            assert!(paused[0].next_run_at.is_none());

            toggle_schedule(ui, &paused[0], true);
            let resumed = collect_project_schedules(ui);
            assert!(resumed[0].effective_enabled);
            assert!(resumed[0].next_run_at.is_some());

            // 4. Switch to Card-creating mode, save, and trigger -> creates a scheduled Card.
            mutate_workflow_file(ui, "scheduled-1.yaml", |doc| {
                if let Some(rule) = doc.on.get_mut("schedule") {
                    rule.card = Some(ScheduleCardConfig {
                        column: Some("develop".into()),
                        title: Some("QA · {{date}} (#{{run}})".into()),
                        body: Some("Automated check.".into()),
                    });
                }
            });
            super::super::save(ui);
            let card_items = collect_project_schedules(ui);
            assert_eq!(
                card_items[0].mode_badge_text(&[
                    ("backlog".into(), "Backlog".into()),
                    ("develop".into(), "Develop".into()),
                ]),
                "Card → Develop"
            );
            trigger_schedule(ui, &card_items[0]);
            ui.snapshot.with_untracked(|s| {
                assert_eq!(s.cards.len(), 1);
                assert_eq!(s.cards[0].stage, "develop");
                assert!(s.cards[0].schedule_source.is_some());
                assert_eq!(s.scheduled_runs.len(), 2);
                assert_eq!(
                    s.scheduled_runs[0].card_id.as_deref(),
                    Some(s.cards[0].id.as_str())
                );
            });
            view().w(px(1200.)).h(px(800.)).child(render(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(800.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<SchedulesProbe>());
        let batch = core.commit().expect("schedules view mounts cleanly");
        let texts: Vec<String> = batch
            .items
            .into_iter()
            .filter_map(|item| match item {
                MountItem::Text { text, .. } => Some(text.text.to_string()),
                _ => None,
            })
            .collect();
        assert!(
            texts.iter().any(|t| t.contains("Scheduled Tasks (1)")),
            "expected mounted Scheduled Tasks header, got {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("Card → Develop")),
            "expected mounted Card → Develop mode badge, got {texts:?}"
        );
    }
}
