//! Creation, YAML quick-edit, and Agent Configuration modals for top-level Schedules.

use crate::presenter::*;
use canban_types::{
    GLOBAL_PROJECT_ID, ScheduleOverlap, ScheduleState,
    cron::{describe_cron, parse_schedule},
    is_global_project_id,
};
use canban_ui_kit::{tokens::*, *};
use canban_workflow_engine::source::{self, ScheduledWorkflowMode, ScheduledWorkflowSpec};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub struct ModalState {
    pub scope: Signal<String>,
    pub name: Signal<String>,
    pub cron: Signal<String>,
    pub timezone: Signal<String>,
    pub overlap: Signal<ScheduleOverlap>,
    pub mode: Signal<String>,
    pub script: Signal<String>,
    pub shell: Signal<String>,
    pub working_directory: Signal<String>,
    pub profile_id: Signal<String>,
    pub instruction: Signal<String>,
    pub card_column: Signal<String>,
    pub card_title: Signal<String>,
    pub card_body: Signal<String>,
    pub error: Signal<String>,
}

impl ModalState {
    pub fn new() -> Self {
        Self {
            scope: signal(GLOBAL_PROJECT_ID.into()),
            name: signal(String::new()),
            cron: signal("0 3 * * *".into()),
            timezone: signal("Local".into()),
            overlap: signal(ScheduleOverlap::Skip),
            mode: signal("script_agent".into()),
            script: signal(String::new()),
            shell: signal("bash".into()),
            working_directory: signal(String::new()),
            profile_id: signal(String::new()),
            instruction: signal(String::new()),
            card_column: signal("develop".into()),
            card_title: signal("Scheduled Task · {{date}}".into()),
            card_body: signal(String::new()),
            error: signal(String::new()),
        }
    }

    pub fn reset_for(&self, ui: Ui, scope: String) {
        let is_global = is_global_project_id(&scope);
        let default_prof = crate::sessions::default_profile(ui);
        let first_col = if !is_global {
            ui.snapshot
                .with_untracked(|s| {
                    s.projects
                        .iter()
                        .find(|p| p.id == scope)
                        .and_then(|p| p.workflow.stages.first())
                        .map(|st| st.id.clone())
                })
                .unwrap_or_else(|| "develop".into())
        } else {
            "develop".into()
        };
        batch(move || {
            self.scope.set(scope);
            self.name.set(String::new());
            self.cron.set("0 3 * * *".into());
            self.timezone.set("Local".into());
            self.overlap.set(ScheduleOverlap::Skip);
            self.mode.set("script_agent".into());
            self.script.set(String::new());
            self.shell.set("bash".into());
            self.working_directory.set(String::new());
            self.profile_id.set(default_prof);
            self.instruction.set(String::new());
            self.card_column.set(first_col);
            self.card_title.set("Scheduled Task · {{date}}".into());
            self.card_body.set(String::new());
            self.error.set(String::new());
        });
    }

    pub fn apply_worktree_gc_preset(&self, ui: Ui) {
        let prof = crate::sessions::default_profile(ui);
        batch(move || {
            self.scope.set(GLOBAL_PROJECT_ID.into());
            self.name.set("Worktree GC (7d)".into());
            self.cron.set("0 3 * * *".into());
            self.timezone.set("Local".into());
            self.overlap.set(ScheduleOverlap::Skip);
            self.mode.set("script_agent".into());
            self.shell.set("bash".into());
            self.script.set(
                "\"$CANBAN_CLI\" worktree gc --merged-before 7d --fail-on-dirty --env-dirty DIRTY_WORKTREES"
                    .into(),
            );
            if self.profile_id.get_untracked().is_empty() && !prof.is_empty() {
                self.profile_id.set(prof);
            }
            self.instruction.set(
                "Worktree GC skipped dirty or active worktrees ($DIRTY_WORKTREES). Inspect the paths and call `\"$CANBAN_CLI\" report --state blocked --message \"...\"` if human confirmation is needed, or finish safe cleanup and report `--state completed`."
                    .into(),
            );
            self.error.set(String::new());
        });
    }
}

impl Default for ModalState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn open_create_modal(ui: Ui, scope: Option<String>) {
    let target_scope = scope
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            let cur = ui.schedules.scope.get_untracked();
            (!cur.is_empty()).then_some(cur)
        })
        .unwrap_or_else(|| GLOBAL_PROJECT_ID.to_owned());
    ui.schedules.modal.reset_for(ui, target_scope);
    ui.schedules.create_modal.set(true);
}

fn validate_modal(ui: Ui) -> Result<Value, String> {
    let m = ui.schedules.modal;
    let raw_scope = m.scope.get_untracked();
    let scope_id = if raw_scope.trim().is_empty() {
        GLOBAL_PROJECT_ID.to_owned()
    } else {
        raw_scope
    };
    let is_global = is_global_project_id(&scope_id);
    let name = m.name.get_untracked().trim().to_owned();
    if name.is_empty() {
        return Err("Enter a schedule name.".into());
    }
    let cron = m.cron.get_untracked().trim().to_owned();
    if cron.is_empty() {
        return Err("Enter a cron expression (e.g. `0 3 * * *`).".into());
    }
    let tz_raw = m.timezone.get_untracked().trim().to_owned();
    let tz_opt = (!tz_raw.is_empty() && !tz_raw.eq_ignore_ascii_case("local")).then_some(tz_raw);
    parse_schedule(&cron, tz_opt.as_deref())?;

    let mut mode = m.mode.get_untracked();
    if is_global && mode == "card" {
        mode = "script_agent".into();
    }
    let script = m.script.get_untracked().trim().to_owned();
    let shell = m.shell.get_untracked().trim().to_owned();
    let working_dir = m.working_directory.get_untracked().trim().to_owned();
    let profile_id = m.profile_id.get_untracked().trim().to_owned();
    let instruction = m.instruction.get_untracked().trim().to_owned();
    if matches!(mode.as_str(), "script" | "script_agent") && script.is_empty() {
        return Err("Enter a shell or CLI command for the script step.".into());
    }

    let mut payload = json!({
        "scope": if is_global { "global" } else { "project" },
        "project_id": scope_id,
        "name": name,
        "cron": cron,
        "timezone": tz_opt,
        "overlap": m.overlap.get_untracked(),
        "mode": mode,
        "enabled": true,
    });
    if !script.is_empty() {
        payload["script"] = json!(script);
    }
    if !shell.is_empty() {
        payload["shell"] = json!(shell);
    }
    if !working_dir.is_empty() {
        payload["working_directory"] = json!(working_dir);
    }
    if !profile_id.is_empty() {
        payload["profile_id"] = json!(profile_id);
    }
    if !instruction.is_empty() {
        payload["instruction"] = json!(instruction);
    }
    if mode == "card" {
        let col = m.card_column.get_untracked().trim().to_owned();
        if !col.is_empty() {
            payload["card_column"] = json!(col);
        }
        let title = m.card_title.get_untracked().trim().to_owned();
        if !title.is_empty() {
            payload["card_title"] = json!(title);
        }
        let body = m.card_body.get_untracked().trim().to_owned();
        if !body.is_empty() {
            payload["card_body"] = json!(body);
        }
    }
    Ok(payload)
}

fn submit_create_modal(ui: Ui) {
    let m = ui.schedules.modal;
    let payload = match validate_modal(ui) {
        Ok(v) => v,
        Err(msg) => {
            m.error.set(msg);
            return;
        }
    };
    m.error.set(String::new());

    if crate::APP.get().is_none() {
        create_offline_schedule(ui, payload);
        return;
    }

    let scope_id = payload["project_id"]
        .as_str()
        .unwrap_or(GLOBAL_PROJECT_ID)
        .to_owned();
    dispatch_complete(ui, "schedule.create", payload, move |res| match res {
        Ok(val) => {
            let sid = val["schedule"]["id"].as_str().map(str::to_owned);
            let fname = val["filename"].as_str().map(str::to_owned);
            ui.schedules.create_modal.set(false);
            if !ui.schedules.scope.get_untracked().is_empty() {
                ui.schedules.scope.set(scope_id.clone());
            }
            if let Some(id) = sid {
                ui.schedules.selected.set(id);
            } else if let Some(f) = fname {
                ui.schedules
                    .selected
                    .set(super::schedule_key(&scope_id, &f, "schedule"));
            }
            ui.notify("Scheduled task created");
        }
        Err(err) => {
            m.error.set(err);
            ui.operation_error.set(String::new());
        }
    });
}

fn create_offline_schedule(ui: Ui, p: Value) {
    let scope_id = p["project_id"]
        .as_str()
        .unwrap_or(GLOBAL_PROJECT_ID)
        .to_owned();
    let is_global = is_global_project_id(&scope_id);
    let name = p["name"].as_str().unwrap_or("Scheduled Task").to_owned();
    let cron = p["cron"].as_str().unwrap_or("0 3 * * *").to_owned();
    let timezone = p["timezone"].as_str().map(str::to_owned);
    let overlap: ScheduleOverlap = serde_json::from_value(p["overlap"].clone()).unwrap_or_default();
    let mode_raw = p["mode"].as_str().unwrap_or("agent");
    let script = p["script"].as_str().unwrap_or("").to_owned();
    let shell = p["shell"].as_str().map(str::to_owned);
    let working_directory = p["working_directory"].as_str().map(str::to_owned);
    let profile = p["profile_id"].as_str().map(str::to_owned);
    let instruction = p["instruction"].as_str().unwrap_or("").to_owned();

    let mode = match mode_raw {
        "script" => ScheduledWorkflowMode::ScriptOnly {
            run: script,
            shell,
            working_directory,
            timeout_minutes: None,
        },
        "script_agent" => ScheduledWorkflowMode::ScriptWithAgentFallback {
            run: script,
            shell,
            working_directory,
            timeout_minutes: None,
            profile: profile.clone(),
            instruction,
        },
        "card" => ScheduledWorkflowMode::CreateCard {
            column: p["card_column"].as_str().map(str::to_owned),
            title: p["card_title"].as_str().map(str::to_owned),
            body: p["card_body"].as_str().map(str::to_owned),
            profile: profile.clone(),
            instruction,
        },
        _ => ScheduledWorkflowMode::AgentOnly {
            profile: profile.clone(),
            instruction,
        },
    };
    let short = canban_types::id();
    let wf_id = if is_global {
        format!("global:schedule-{short}")
    } else {
        format!("schedule:{scope_id}:{short}")
    };
    let doc = source::build_scheduled_workflow_document(ScheduledWorkflowSpec {
        workflow_id: wf_id.clone(),
        name: name.clone(),
        scope: if is_global {
            canban_types::WorkflowScope::Global
        } else {
            canban_types::WorkflowScope::Project
        },
        cron: cron.clone(),
        timezone: timezone.clone(),
        overlap,
        enabled: true,
        mode,
    });
    let Ok(yaml) = source::encode(&doc) else {
        ui.schedules
            .modal
            .error
            .set("Failed to encode Workflow YAML".into());
        return;
    };
    let now_ms = canban_types::now();
    let next_run_at = parse_schedule(&cron, timezone.as_deref())
        .ok()
        .and_then(|(sched, tz)| sched.next_after_ms(now_ms, tz));
    let mut selected_key = String::new();
    ui.snapshot.update(|s| {
        let existing: BTreeSet<String> = s
            .workflow_sources
            .iter()
            .filter(|ws| ws.project_id == scope_id)
            .map(|ws| ws.filename.clone())
            .collect();
        let mut idx = 1usize;
        let mut fname = format!("scheduled-{idx}.yaml");
        while existing.contains(&fname) {
            idx += 1;
            fname = format!("scheduled-{idx}.yaml");
        }
        let sid = format!("schedule:{scope_id}:{wf_id}:schedule");
        selected_key = super::schedule_key(&scope_id, &fname, "schedule");
        let _ = super::push_offline_workflow_source(s, &scope_id, &fname, &yaml);
        s.schedule_states.push(ScheduleState {
            id: sid,
            project_id: scope_id.clone(),
            workflow_id: wf_id,
            workflow_name: name,
            filename: fname,
            rule_id: "schedule".into(),
            cron,
            timezone,
            overlap,
            creates_card: mode_raw == "card",
            starts_with_script: matches!(mode_raw, "script" | "script_agent"),
            target_column: p["card_column"].as_str().map(str::to_owned),
            profile_id: profile,
            enabled: true,
            last_scheduled_at: None,
            next_run_at,
            last_run_id: None,
            run_count: 0,
            queued_triggers: 0,
            updated_at: now_ms,
        });
    });
    ui.schedules.create_modal.set(false);
    if !ui.schedules.scope.get_untracked().is_empty() {
        ui.schedules.scope.set(scope_id);
    }
    ui.schedules.selected.set(selected_key);
    ui.notify("Scheduled task created");
}

pub fn describe_with_agent(ui: Ui, scope: String, custom_seed: Option<String>) {
    let target_scope = if scope.trim().is_empty() {
        GLOBAL_PROJECT_ID.to_owned()
    } else {
        scope
    };
    ui.schedules.agent_scope.set(target_scope.clone());
    let seed = custom_seed.unwrap_or_else(|| {
        if is_global_project_id(&target_scope) {
            "Configure a Global (`scope: global`) scheduled workflow in Canban. Ask me for cadence and actions if unclear, then author clean schema 9 YAML with `workflow.apply` targeting `__global`.".into()
        } else {
            format!(
                "Configure a scheduled workflow for Project `{target_scope}`. Ask me for cadence and mode (`script` + `on_failure -> agent`, `agent`, or `card`), then update via `workflow.apply`."
            )
        }
    });
    ui.schedules.agent_prompt.set(seed);
    ui.schedules.agent_open.set(true);
    if crate::APP.get().is_none() {
        return;
    }
    ui.spawn(async move {
        let res = blocking(move || {
            crate::routed_call(
                "workflow.prompt",
                json!({ "project_id": target_scope }),
                None,
            )
        })
        .await;
        if let Ok(Ok(val)) = res
            && let Some(prompt) = val["prompt"].as_str()
            && ui.schedules.agent_prompt.get_untracked().len() < 400
        {
            let existing = ui.schedules.agent_prompt.get_untracked();
            ui.schedules
                .agent_prompt
                .set(format!("{existing}\n\n---\n\n{prompt}"));
        }
    });
}

fn launch_agent_config(ui: Ui, fresh: bool) {
    let scope = ui.schedules.agent_scope.get_untracked();
    let prompt = ui.schedules.agent_prompt.get_untracked();
    let profile = crate::sessions::default_profile(ui);
    if profile.is_empty() {
        ui.notify("Configure a default Agent profile in Settings first.");
        return;
    }
    dispatch_complete(
        ui,
        "configuration.open",
        json!({
            "project_id": scope,
            "profile_id": profile,
            "prompt": prompt,
            "fresh": fresh,
            "auto_fresh": !fresh,
        }),
        move |res| match res {
            Ok(v) => {
                ui.schedules.agent_open.set(false);
                if let Some(tid) = v["terminal_id"].as_str() {
                    ui.open_terminal(tid.to_owned());
                } else {
                    ui.notify("Opened configuration Agent in Terminal");
                }
            }
            Err(e) => {
                ui.notify(e);
            }
        },
    );
}

pub fn create_modal(ui: Ui) -> impl IntoElement {
    let m = ui.schedules.modal;
    let cron_presets = [
        ("Hourly", "0 * * * *"),
        ("Daily 03:00", "0 3 * * *"),
        ("Weekdays 09:00", "0 9 * * 1-5"),
        ("Monday 09:00", "0 9 * * 1"),
    ];
    let tz_presets = [("Local", "Local"), ("UTC", "UTC")];
    let cron_preview = computed(move || {
        let raw = m.cron.get();
        let tz = m.timezone.get();
        let tz_opt = (!tz.trim().is_empty() && !tz.trim().eq_ignore_ascii_case("local"))
            .then_some(tz.trim().to_owned());
        match parse_schedule(raw.trim(), tz_opt.as_deref()) {
            Ok((sched, p_tz)) => {
                let desc = describe_cron(raw.trim());
                let next = sched
                    .next_after_ms(canban_types::now(), p_tz)
                    .map(|ts| format!(" · Next: {}", super::format_run_ts(ts, tz_opt.as_deref())))
                    .unwrap_or_default();
                Ok(format!("{desc}{next}"))
            }
            Err(e) => Err(e),
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
                    .justify_between()
                    .gap_2()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(icon_tinted(Icon::Clock, ACCENT))
                            .child(heading("New Scheduled Task")),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(compact_action_with_icon(
                                Icon::Agent,
                                "Describe with agent…",
                                Variant::Secondary,
                                move |_| {
                                    let scope = m.scope.get_untracked();
                                    let draft_desc = format!(
                                        "Create or customize a scheduled task named \"{}\" (cron: `{}`, mode: `{}`). Ask clarifying questions if needed and apply YAML cleanly.",
                                        m.name.get_untracked(),
                                        m.cron.get_untracked(),
                                        m.mode.get_untracked()
                                    );
                                    ui.schedules.create_modal.set(false);
                                    describe_with_agent(ui, scope, Some(draft_desc));
                                },
                            ))
                            .child(compact_action(
                                "Template: Worktree GC (7d)",
                                Variant::Secondary,
                                move |_| {
                                    m.apply_worktree_gc_preset(ui);
                                },
                            ))
                            .child(chrome_button(Icon::Close, "Close modal", move |_| {
                                ui.schedules.create_modal.set(false)
                            })),
                    ),
            )
            .child(
                scroll().flex_1().min_h(px(0.)).child(
                    view()
                        .gap_3()
                        .pr_2()
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .gap_3()
                                .child(
                                    view()
                                        .w(px(260.))
                                        .gap_1p5()
                                        .child(muted("Scope"))
                                        .child(search_popover(
                                            press_target()
                                                .h(px(32.))
                                                .px_2p5()
                                                .rounded(px(6.))
                                                .border_1()
                                                .border_color(BORDER)
                                                .bg(INSET)
                                                .hover(|s| s.bg(CARD_HOVER))
                                                .flex_row()
                                                .items_center()
                                                .justify_between()
                                                .gap_2()
                                                .child(
                                                    label(move || {
                                                        let sid = m.scope.get();
                                                        ui.snapshot.with(|s| super::scope_display_name(s, &sid))
                                                    })
                                                    .truncate()
                                                    .min_w(px(0.)),
                                                )
                                                .child(icon(Icon::ChevronDown).w(px(12.)).h(px(12.))),
                                            move || {
                                                let cur = m.scope.get();
                                                ui.snapshot.with(|s| {
                                                    let mut opts = Vec::with_capacity(s.projects.len() + 1);
                                                    opts.push(
                                                        PickerOption::new(GLOBAL_PROJECT_ID, "Global · Host")
                                                            .detail("Host-level automation (no Card / Project)")
                                                            .icon(Icon::Server)
                                                            .selected(is_global_project_id(&cur)),
                                                    );
                                                    for p in &s.projects {
                                                        opts.push(
                                                            PickerOption::new(p.id.clone(), p.name.clone())
                                                                .group("Projects")
                                                                .icon(Icon::Folder)
                                                                .selected(cur == p.id),
                                                        );
                                                    }
                                                    opts
                                                })
                                            },
                                            move |picked| {
                                                m.scope.set(picked.clone());
                                                if is_global_project_id(&picked)
                                                    && m.mode.get_untracked() == "card"
                                                {
                                                    m.mode.set("script_agent".into());
                                                } else if !is_global_project_id(&picked) {
                                                    let first = ui.snapshot.with_untracked(|s| {
                                                        s.projects
                                                            .iter()
                                                            .find(|p| p.id == picked)
                                                            .and_then(|p| p.workflow.stages.first())
                                                            .map(|st| st.id.clone())
                                                    });
                                                    if let Some(col) = first {
                                                        m.card_column.set(col);
                                                    }
                                                }
                                            },
                                        )),
                                )
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(240.))
                                        .gap_1p5()
                                        .child(muted("Schedule Name"))
                                        .child(field(
                                            m.name,
                                            "e.g. Worktree GC (7d), Dependency Audit…",
                                        )),
                                ),
                        )
                        .child(
                            view()
                                .gap_1p5()
                                .child(muted("Cron Schedule"))
                                .child(view().flex_row().flex_wrap().gap_1().children(
                                    cron_presets.into_iter().map(|(lbl, expr)| {
                                        let expr_s = expr.to_owned();
                                        compact_action(
                                            lbl,
                                            if m.cron.get_untracked() == expr {
                                                Variant::Primary
                                            } else {
                                                Variant::Secondary
                                            },
                                            move |_| m.cron.set(expr_s.clone()),
                                        )
                                    }),
                                ))
                                .child(field(m.cron, "5-field POSIX cron (e.g. 0 3 * * *)"))
                                .child(dynamic(move || match cron_preview.get() {
                                    Ok(text) => muted(text).into_element(),
                                    Err(err) => notice(err, Tone::Danger).into_element(),
                                })),
                        )
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .gap_4()
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(220.))
                                        .gap_1p5()
                                        .child(muted("Timezone"))
                                        .child(view().flex_row().flex_wrap().gap_1().children(
                                            tz_presets.into_iter().map(|(lbl, val)| {
                                                let v = val.to_owned();
                                                compact_action(lbl, Variant::Secondary, move |_| {
                                                    m.timezone.set(v.clone())
                                                })
                                            }),
                                        ))
                                        .child(field(m.timezone, "Local, UTC, Asia/Shanghai…")),
                                )
                                .child(
                                    view()
                                        .min_w(px(220.))
                                        .gap_1p5()
                                        .child(muted("Overlap Policy"))
                                        .child(
                                            segmented()
                                                .child(compact_segment(
                                                    "Skip",
                                                    None,
                                                    move || m.overlap.get() == ScheduleOverlap::Skip,
                                                    move |_| m.overlap.set(ScheduleOverlap::Skip),
                                                ))
                                                .child(compact_segment(
                                                    "Queue",
                                                    None,
                                                    move || m.overlap.get() == ScheduleOverlap::Queue,
                                                    move |_| m.overlap.set(ScheduleOverlap::Queue),
                                                ))
                                                .child(compact_segment(
                                                    "Allow",
                                                    None,
                                                    move || m.overlap.get() == ScheduleOverlap::Allow,
                                                    move |_| m.overlap.set(ScheduleOverlap::Allow),
                                                )),
                                        ),
                                ),
                        )
                        .child(
                            view()
                                .gap_2()
                                .pt_2()
                                .border_t_1()
                                .border_color(BORDER)
                                .child(muted("Execution Mode"))
                                .child(
                                    segmented()
                                        .child(compact_segment(
                                            "Script Only",
                                            Some(Icon::Terminal),
                                            move || m.mode.get() == "script",
                                            move |_| m.mode.set("script".into()),
                                        ))
                                        .child(compact_segment(
                                            "Script → Agent",
                                            Some(Icon::Zap),
                                            move || m.mode.get() == "script_agent",
                                            move |_| m.mode.set("script_agent".into()),
                                        ))
                                        .child(compact_segment(
                                            "Terminal Agent",
                                            Some(Icon::Agent),
                                            move || m.mode.get() == "agent",
                                            move |_| m.mode.set("agent".into()),
                                        ))
                                        .child(when(
                                            move || !is_global_project_id(&m.scope.get()),
                                            move || {
                                                compact_segment(
                                                    "Create Card",
                                                    Some(Icon::Board),
                                                    move || m.mode.get() == "card",
                                                    move |_| m.mode.set("card".into()),
                                                )
                                            },
                                        )),
                                )
                                .child(when(
                                    move || is_global_project_id(&m.scope.get()),
                                    move || {
                                        muted("Global schedules run script & agent automation directly without consuming an Agent slot on clean script exits (exit 0).")
                                    },
                                ))
                                .child(when(
                                    move || matches!(m.mode.get().as_str(), "script" | "script_agent"),
                                    move || {
                                        view()
                                            .p_3()
                                            .gap_2()
                                            .rounded(px(RADIUS))
                                            .bg(INSET)
                                            .child(
                                                view()
                                                    .flex_row()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap_2()
                                                    .child(label("Script Command").font_weight(FontWeight::SEMIBOLD))
                                                    .child(field(m.shell, "Shell (default bash)").w(px(150.))),
                                            )
                                            .child(field(
                                                m.script,
                                                "e.g. \"$CANBAN_CLI\" worktree gc --merged-before 7d --fail-on-dirty --env-dirty DIRTY_WORKTREES",
                                            ))
                                    },
                                ))
                                .child(when(
                                    move || matches!(m.mode.get().as_str(), "script_agent" | "agent"),
                                    move || {
                                        let profiles = ui.profiles.get();
                                        view()
                                            .p_3()
                                            .gap_2()
                                            .rounded(px(RADIUS))
                                            .bg(INSET)
                                            .child(
                                                label(move || {
                                                    if m.mode.get() == "script_agent" {
                                                        "Fallback Agent (runs only when script exits non-zero; keeps PTY alive on --state blocked)"
                                                    } else {
                                                        "Terminal Agent"
                                                    }
                                                })
                                                .font_weight(FontWeight::SEMIBOLD),
                                            )
                                            .child(
                                                view().flex_row().flex_wrap().gap_1p5().children(
                                                    profiles.into_iter().map(|prof| {
                                                        let pid = prof.id.clone();
                                                        let click_pid = pid.clone();
                                                        compact_action(
                                                            format!("@{}", prof.id),
                                                            if m.profile_id.get() == pid {
                                                                Variant::Primary
                                                            } else {
                                                                Variant::Secondary
                                                            },
                                                            move |_| m.profile_id.set(click_pid.clone()),
                                                        )
                                                    }),
                                                ),
                                            )
                                            .child(field(
                                                m.instruction,
                                                "Instructions for the Agent (reports via `$CANBAN_CLI report --state blocked|completed`)…",
                                            ))
                                    },
                                ))
                                .child(when(
                                    move || m.mode.get() == "card",
                                    move || {
                                        view()
                                            .p_3()
                                            .gap_2()
                                            .rounded(px(RADIUS))
                                            .bg(INSET)
                                            .child(label("Board Card Creation").font_weight(FontWeight::SEMIBOLD))
                                            .child(field(m.card_column, "Target column (e.g. develop)"))
                                            .child(field(
                                                m.card_title,
                                                "Card title ({{date}}, {{run_number}}, {{workflow}})",
                                            ))
                                            .child(field(m.card_body, "Optional Card body template…"))
                                    },
                                )),
                        ),
                ),
            )
            .child(when(
                move || !m.error.get().is_empty(),
                move || notice(move || m.error.get(), Tone::Danger),
            ))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(action("Cancel", Variant::Quiet, move |_| {
                        ui.schedules.create_modal.set(false);
                    }))
                    .child(action("Create Schedule", Variant::Primary, move |_| {
                        submit_create_modal(ui);
                    })),
            ),
        move || ui.schedules.create_modal.set(false),
    )
}

pub fn open_yaml_editor(ui: Ui, scope_id: String, filename: String) {
    let local_draft = if !is_global_project_id(&scope_id) {
        ui.workflows.drafts.with_untracked(|d| {
            d.get(&scope_id)
                .and_then(|dr| dr.sources.get(&filename).cloned())
        })
    } else {
        None
    };
    let snap_src = ui.snapshot.with_untracked(|s| {
        crate::workflows::project_template_files_from_snapshot(s, &scope_id)
            .into_iter()
            .find(|ws| ws.filename == filename)
            .map(|ws| ws.source)
    });
    let initial = local_draft.or(snap_src).unwrap_or_default();
    ui.schedules.yaml_scope.set(scope_id.clone());
    ui.schedules.yaml_filename.set(filename);
    ui.schedules.yaml_revision.set(String::new());
    ui.schedules.yaml_source.set(initial);
    ui.schedules.yaml_error.set(String::new());
    ui.schedules.yaml_open.set(true);

    if crate::APP.get().is_none() {
        return;
    }
    let origin_scope = scope_id.clone();
    ui.spawn(async move {
        let res = blocking(move || {
            crate::routed_call(
                "workflow.sources",
                json!({ "project_id": origin_scope }),
                None,
            )
        })
        .await;
        if let Ok(Ok(val)) = res {
            let rev = val["revision"].as_str().unwrap_or("").to_owned();
            ui.schedules.yaml_revision.set(rev);
            if let Some(arr) = val["sources"].as_array()
                && let Some(src) = arr
                    .iter()
                    .find(|s| s["filename"] == ui.schedules.yaml_filename.get_untracked())
                    .and_then(|s| s["source"].as_str())
            {
                ui.schedules.yaml_source.set(src.to_owned());
            }
        }
    });
}

fn save_yaml_modal(ui: Ui) {
    let scope = ui.schedules.yaml_scope.get_untracked();
    let filename = ui.schedules.yaml_filename.get_untracked();
    let body = ui.schedules.yaml_source.get_untracked();
    if let Err(e) = source::parse(&body) {
        ui.schedules.yaml_error.set(format!("YAML error: {e}"));
        return;
    }
    if crate::APP.get().is_none() {
        ui.snapshot.update(|s| {
            let _ = super::push_offline_workflow_source(s, &scope, &filename, &body);
        });
        ui.schedules.yaml_open.set(false);
        ui.notify("Workflow YAML updated");
        return;
    }
    let s_for_rpc = scope.clone();
    let f_for_rpc = filename.clone();
    let rev = ui.schedules.yaml_revision.get_untracked();
    ui.spawn(async move {
        let res = blocking(move || {
            let col =
                crate::routed_call("workflow.sources", json!({ "project_id": s_for_rpc }), None)?;
            let actual_rev = if rev.is_empty() {
                col["revision"].as_str().unwrap_or("").to_owned()
            } else {
                rev
            };
            let mut inputs: Vec<Value> = col["sources"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|item| item["filename"] != f_for_rpc)
                .map(|item| json!({"filename": item["filename"], "source": item["source"]}))
                .collect();
            inputs.push(json!({"filename": f_for_rpc, "source": body}));
            crate::routed_call(
                "workflow.apply",
                json!({
                    "project_id": s_for_rpc,
                    "revision": actual_rev,
                    "sources": inputs,
                    "request_key": canban_types::id(),
                }),
                None,
            )
        })
        .await;
        match res {
            Ok(Ok(_)) => {
                ui.schedules.yaml_open.set(false);
                ui.notify("Workflow YAML saved and published");
            }
            Ok(Err(e)) => ui.schedules.yaml_error.set(format!("{e:#}")),
            Err(e) => ui.schedules.yaml_error.set(e.to_string()),
        }
    });
}

pub fn yaml_modal(ui: Ui) -> impl IntoElement {
    let s = ui.schedules;
    let editor = canban_ui_kit::markdown::MarkdownInput::new();
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
                    .child(icon_tinted(Icon::Trigger, ACCENT))
                    .child(heading(move || s.yaml_filename.get()))
                    .child(badge(move || {
                        if is_global_project_id(&s.yaml_scope.get()) {
                            "Global · Host"
                        } else {
                            "Project"
                        }
                    }))
                    .child(view().flex_1())
                    .child(chrome_button(Icon::Close, "Close YAML editor", move |_| {
                        s.yaml_open.set(false)
                    })),
            )
            .child(
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .bg(INSET)
                    .rounded(px(RADIUS))
                    .p_2()
                    .child(
                        editor
                            .source(
                                s.yaml_source,
                                "Schedule Workflow YAML",
                                "Write Workflow YAML…",
                            )
                            .language(Some("yaml".into()))
                            .highlighting(true)
                            .line_numbers(true)
                            .indent_guides(true)
                            .tab_size(Some(2)),
                    ),
            )
            .child(when(
                move || !s.yaml_error.get().is_empty(),
                move || notice(move || s.yaml_error.get(), Tone::Danger),
            ))
            .child(
                view()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(action("Cancel", Variant::Quiet, move |_| {
                        s.yaml_open.set(false);
                    }))
                    .child(action("Save & Publish", Variant::Primary, move |_| {
                        save_yaml_modal(ui);
                    })),
            ),
        move || s.yaml_open.set(false),
    )
}

pub fn agent_modal(ui: Ui) -> impl IntoElement {
    let s = ui.schedules;
    let editor = canban_ui_kit::markdown::MarkdownInput::new();
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
                    .child(icon_tinted(Icon::Agent, ACCENT))
                    .child(heading("Configure Schedule with Agent"))
                    .child(badge(move || {
                        let scope = s.agent_scope.get();
                        ui.snapshot.with(|snap| super::scope_display_name(snap, &scope))
                    }))
                    .child(view().flex_1())
                    .child(chrome_button(Icon::Close, "Close dialog", move |_| {
                        s.agent_open.set(false)
                    })),
            )
            .child(
                muted("Edit or review this prompt, then launch an Agent session in Terminal to generate and publish the scheduled workflow YAML."),
            )
            .child(
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .bg(INSET)
                    .rounded(px(RADIUS))
                    .p_2()
                    .child(
                        editor
                            .source(
                                s.agent_prompt,
                                "Configuration Prompt",
                                "Describe your schedule…",
                            )
                            .language(Some("markdown".into()))
                            .soft_wrap(true)
                            .line_numbers(false),
                    ),
            )
            .child(
                view()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(action("Cancel", Variant::Quiet, move |_| {
                        s.agent_open.set(false)
                    }))
                    .child(action("New Conversation", Variant::Secondary, move |_| {
                        launch_agent_config(ui, true)
                    }))
                    .child(action("Open in Terminal", Variant::Primary, move |_| {
                        launch_agent_config(ui, false)
                    })),
            ),
        move || s.agent_open.set(false),
    )
}
