//! Project Workflows owns source drafts. Source editors and Prompt preview use
//! native NSH selection/editing; launching is a separate explicit intent.
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub base: Value,
    pub sources: BTreeMap<String, String>,
    pub selected: String,
}
impl Draft {
    fn dirty(&self) -> bool {
        self.inputs()
            != self.base["sources"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|v| json!({"filename":v["filename"],"source":v["source"]}))
                .collect::<Vec<_>>()
    }
    fn needs_apply(&self) -> bool {
        self.dirty()
            || self.base["sources"]
                .as_array()
                .is_some_and(|files| files.iter().any(|f| f["unpublished"] == true))
    }
    fn inputs(&self) -> Vec<Value> {
        self.sources
            .iter()
            .map(|(name, source)| json!({"filename":name,"source":source}))
            .collect()
    }
    fn accept_publication(&mut self, value: Value, submitted: &BTreeMap<String, String>) {
        if &self.sources == submitted {
            self.accept(value);
        } else {
            self.base = value;
        }
    }
    fn accept(&mut self, value: Value) {
        self.sources = value["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| Some((v["filename"].as_str()?.into(), v["source"].as_str()?.into())))
            .collect();
        if !self.sources.contains_key(&self.selected) {
            self.selected = self.sources.keys().next().cloned().unwrap_or_default();
        }
        self.base = value;
    }
}
#[derive(Clone, Copy)]
pub struct State {
    pub drafts: Signal<BTreeMap<String, Draft>>,
    pub selected: Signal<String>,
    pub mode: Signal<String>,
    pub selected_step: Signal<String>,
    pub new_column_name: Signal<String>,
    pub error: Signal<String>,
    pub latest: Signal<Value>,
    pub agent: Signal<bool>,
    pub prompt: Signal<String>,
    pub profile: Signal<String>,
    pub copy_status: Signal<String>,
    pub loading: Signal<bool>,
    pub history: Signal<Vec<canban_types::WorkflowRevision>>,
    pub history_open: Signal<bool>,
    pub history_revision: Signal<u64>,
    pub history_name: Signal<String>,
    pub epoch: Signal<u64>,
}
impl State {
    pub fn new() -> Self {
        Self {
            drafts: signal(BTreeMap::new()),
            selected: signal(String::new()),
            mode: signal("visual".into()),
            selected_step: signal(String::new()),
            new_column_name: signal(String::new()),
            error: signal(String::new()),
            latest: signal(Value::Null),
            agent: signal(false),
            prompt: signal(String::new()),
            profile: signal(String::new()),
            copy_status: signal(String::new()),
            loading: signal(false),
            history: signal(Vec::new()),
            history_open: signal(false),
            history_revision: signal(0),
            history_name: signal(String::new()),
            epoch: signal(0),
        }
    }
}
impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}
fn load(ui: Ui, discard: bool) {
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    let project = ui.project.get_untracked();
    let state = ui.workflows;
    let ticket = state.epoch.get_untracked() + 1;
    state.epoch.set(ticket);
    state.loading.set(true);
    ui.spawn(async move {
        let id = project.clone();
        let result = blocking(move || app.call("workflow.sources", json!({"project_id":id}))).await;
        if state.epoch.get_untracked() != ticket {
            return;
        }
        state.loading.set(false);
        match result {
            Ok(Ok(v)) => {
                let dirty = state
                    .drafts
                    .with_untracked(|d| d.get(&project).is_some_and(Draft::dirty));
                if dirty && !discard {
                    let same = state.drafts.with_untracked(|d| {
                        d.get(&project)
                            .is_some_and(|d| d.base["revision"] == v["revision"])
                    });
                    state.latest.set(if same { Value::Null } else { v });
                } else {
                    state
                        .drafts
                        .update(|d| d.entry(project.clone()).or_default().accept(v));
                    state.latest.set(Value::Null);
                    state.error.set(String::new());
                }
                if ui.project.get_untracked() == project {
                    state.selected.set(state.drafts.with_untracked(|d| {
                        d.get(&project)
                            .map(|d| d.selected.clone())
                            .unwrap_or_default()
                    }));
                }
            }
            Ok(Err(e)) => state.error.set(format!("{e:#}")),
            Err(e) => state.error.set(e.to_string()),
        }
    });
}
fn save(ui: Ui) {
    let state = ui.workflows;
    let project = ui.project.get_untracked();
    let Some(draft) = state.drafts.with_untracked(|d| d.get(&project).cloned()) else {
        return;
    };
    if ui.is_method_pending("workflow.apply")
        || state.loading.get_untracked()
        || !draft.needs_apply()
    {
        return;
    }
    state.error.set(String::new());
    dispatch_complete(
        ui,
        "workflow.apply",
        json!({"project_id":project,"revision":draft.base["revision"],"sources":draft.inputs()}),
        move |r| match r {
            Ok(v) => {
                state.epoch.update(|n| *n += 1);
                state.loading.set(false);
                state.drafts.update(|d| {
                    let current = d.entry(project.clone()).or_default();
                    current.accept_publication(v, &draft.sources);
                });
                if ui.project.get_untracked() == project {
                    state.latest.set(Value::Null);
                    ui.notify("Workflow saved");
                }
            }
            Err(e) => {
                if ui.project.get_untracked() == project {
                    state.error.set(e);
                    ui.operation_error.set(String::new());
                    load(ui, false);
                }
            }
        },
    );
}
fn edit_agent(ui: Ui) {
    let state = ui.workflows;
    let Some(app) = crate::APP.get().cloned() else {
        state.agent.set(true);
        return;
    };
    let origin = ui.location();
    let project = ui.project.get_untracked();
    state.copy_status.set(String::new());
    if state.profile.get_untracked().is_empty() {
        state.profile.set(ui.profiles.with_untracked(|p| {
            p.iter()
                .find(|p| p.id == "codex")
                .or_else(|| p.first())
                .map(|p| p.id.clone())
                .unwrap_or_default()
        }));
    }
    ui.spawn(async move {
        let result =
            blocking(move || app.call("workflow.prompt", json!({"project_id":project}))).await;
        if ui.location() != origin {
            return;
        }
        match result {
            Ok(Ok(v)) => {
                state
                    .prompt
                    .set(v["prompt"].as_str().unwrap_or_default().into());
                state.agent.set(true);
            }
            Ok(Err(e)) => state.error.set(format!("{e:#}")),
            Err(e) => state.error.set(e.to_string()),
        }
    });
}
fn launch(ui: Ui, fresh: bool) {
    let origin = ui.location();
    let state = ui.workflows;
    state.error.set(String::new());
    dispatch_complete(
        ui,
        "configuration.open",
        json!({"project_id":ui.project.get_untracked(),"profile_id":state.profile.get_untracked(),"prompt":state.prompt.get_untracked(),"fresh":fresh}),
        move |result| {
            if ui.location() != origin {
                return;
            }
            match result {
                Ok(value) => {
                    if let Some(id) = value["terminal_id"].as_str() {
                        state.agent.set(false);
                        state.history_open.set(false);
                        ui.open_terminal(id.into());
                    }
                }
                Err(error) => {
                    state.error.set(error);
                    ui.operation_error.set(String::new());
                }
            }
        },
    );
}
fn copy_prompt(ui: Ui) {
    let state = ui.workflows;
    let prompt = state.prompt.get_untracked();
    ui.spawn(async move {
        let result = blocking(move || -> anyhow::Result<()> {
            use std::io::Write;
            let mut child = std::process::Command::new("/usr/bin/pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()?;
            child.stdin.take().unwrap().write_all(prompt.as_bytes())?;
            anyhow::ensure!(child.wait()?.success(), "Clipboard copy failed");
            Ok(())
        })
        .await;
        state.copy_status.set(match result {
            Ok(Ok(())) => "Copied".into(),
            _ => "Copy failed. Select the Prompt below and copy it manually.".into(),
        });
    });
}
pub fn screen(ui: Ui) -> impl IntoElement {
    let state = ui.workflows;
    state.agent.set(false);
    state.history_open.set(false);
    state.error.set(String::new());
    let project = ui.project.get_untracked();
    state.selected.set(state.drafts.with_untracked(|d| {
        d.get(&project)
            .map(|d| d.selected.clone())
            .unwrap_or_default()
    }));
    let changed = computed(move || {
        let project = ui.project.get();
        ui.snapshot.with(|s| {
            s.workflow_sources
                .iter()
                .filter(|f| f.project_id == project)
                .map(|f| (f.id.clone(), f.revision, f.hash.clone()))
                .collect::<Vec<_>>()
        })
    });
    effect(move || {
        let _ = changed.get();
        load(ui, false);
    });
    let names = computed(move || {
        let project = ui.project.get();
        state.drafts.with(|d| {
            d.get(&project)
                .map(|d| d.sources.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    let apply = computed(move || {
        let project = ui.project.get();
        state
            .drafts
            .with(|d| d.get(&project).is_some_and(Draft::needs_apply))
    });
    let dirty = computed(move || {
        let project = ui.project.get();
        state
            .drafts
            .with(|d| d.get(&project).is_some_and(Draft::dirty))
    });
    on_key_with(
        "mod+s",
        KeyOptions {
            when: nsh_core::keymap::When::Always,
            ..Default::default()
        },
        move |_| {
            if !state.agent.get_untracked()
                && !state.history_open.get_untracked()
                && !ui.quick_open.get_untracked()
            {
                save(ui);
            }
        },
    );
    view().flex_1().min_h(px(0.)).min_w(px(0.)).p_5().gap_4()
        .child(view().flex_row().items_center().gap_3()
            .child(chrome_button(Icon::Back, "Back to Board", move |_| ui.navigate("board")))
            .child(heading(move || format!("{} / Workflows", ui.current_project().map(|p| p.name).unwrap_or_default())).truncate().min_w(px(0.)).flex_1())
            .child(action_with_icon(Icon::Agent, "Edit with agent", Variant::Primary, move |_| edit_agent(ui)).flex_shrink_0().disabled(move || ui.pending.get() || state.loading.get())))
        .child(when(move || !state.error.get().is_empty(), move || notice(move || state.error.get(), Tone::Attention)))
        .child(view().flex_1().min_h(px(0.)).flex_row().gap_4()
            .child(view().w(px(200.)).flex_shrink_0().min_h(px(0.)).gap_2()
                .child(scroll().min_h(px(0.)).child(view().gap_1().child(each(move || names.get(), |name| name.clone(), move |name, _| {
                    let name = name.get_untracked();
                    let selected = name.clone();
                    let select = name.clone();
                    let title = name.clone();
                    let dirty_name = name.clone();
                    action_with_icon(Icon::Trigger, move || {
                        let project = ui.project.get();
                        let unsaved = state.drafts.with(|d| d.get(&project).is_some_and(|d| source_dirty(d, &title)));
                        format!("{}{}", title, if unsaved { " •" } else { "" })
                    }, Variant::Quiet, move |_| {
                        let project = ui.project.get_untracked();
                        state.drafts.update(|d| { if let Some(d) = d.get_mut(&project) { d.selected = select.clone(); } });
                        state.selected.set(select.clone());
                    }).w_full().items_center().justify_start()
                        .selected(move || state.selected.get() == selected)
                        .accessibility_label(move || {
                            let project = ui.project.get();
                            let unsaved = state.drafts.with(|d| d.get(&project).is_some_and(|d| source_dirty(d, &dirty_name)));
                            Some(format!("{}{}", dirty_name, if unsaved { " · Unsaved changes" } else { "" }).into())
                        })
                        .bg(move || if state.selected.get() == name { SELECTED } else { CLEAR })
                }))))
                .child(action_with_icon(Icon::Plus, "New workflow", Variant::Quiet, move |_| new_workflow(ui)).w_full().justify_start().disabled(move || ui.pending.get() || state.loading.get())))
            .child(view().flex_1().min_w(px(0.)).min_h(px(0.)).gap_2()
                .child(view().flex_row().items_center().gap_2()
                    .child(label(move || state.selected.get()).truncate().min_w(px(0.)).flex_1())
                    .child(
                        segmented()
                            .child(segment(
                                "Visual Studio",
                                Some(Icon::Trigger),
                                move || state.mode.get() != "yaml",
                                move |_| state.mode.set("visual".into()),
                            ))
                            .child(segment(
                                "YAML Source",
                                Some(Icon::Edit),
                                move || state.mode.get() == "yaml",
                                move |_| state.mode.set("yaml".into()),
                            )),
                    )
                    .child(chrome_button_disabled(Icon::History, "Workflow history", move || {
                        let project = ui.project.get(); let name = state.selected.get();
                        !state.drafts.with(|d| d.get(&project).and_then(|d| d.base["sources"].as_array()).is_some_and(|rows| rows.iter().any(|r| r["filename"] == name && r["active"]["revision"].as_u64().is_some_and(|v| v > 0))))
                    }, move |_| show_history(ui)))
                    .child(when(move || dirty.get() || !state.latest.get().is_null(), move || chrome_button_disabled(Icon::Undo, "Discard changes and reload", move || ui.pending.get() || state.loading.get(), move |_| load(ui, true)))))
                .child(when(move || !state.latest.get().is_null(), move || view().gap_2()
                    .child(muted("Workflows changed elsewhere. Your edits are kept; discard to load the saved sources."))
                    .child(view().h(px(150.)).bg(INSET).rounded(px(RADIUS)).p_2().child(
                        nsh_base::editor().value(move || Some(state.latest.with(|v| v["sources"].as_array().into_iter().flatten().find(|f| f["filename"] == state.selected.get()).and_then(|f| f["source"].as_str()).unwrap_or("# This source is absent from the latest saved collection.").to_owned()).into()))
                            .language(Some("yaml".into())).read_only(true).soft_wrap(true).line_numbers(false).folding(false).accessibility_label("Latest saved Workflow YAML").size_full()))))
                .child(dynamic(move || {
                    if state.mode.get() == "yaml" || !state.latest.get().is_null() {
                        source_editor(ui)
                    } else {
                        visual_studio(ui)
                    }
                }))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .rounded(px(RADIUS))
                        .bg(move || if apply.get() { SELECTED } else { CLEAR })
                        .border_1()
                        .border_color(move || if apply.get() { ACCENT } else { CLEAR })
                        .child(muted(move || {
                            if apply.get() {
                                "Unsaved workflow changes · Press Save or ⌘S to apply"
                            } else {
                                "All workflow changes saved"
                            }
                        }))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(when(
                                    move || dirty.get() || !state.latest.get().is_null(),
                                    move || {
                                        action("Discard", Variant::Quiet, move |_| load(ui, true))
                                            .disabled(move || {
                                                ui.is_method_pending("workflow.apply")
                                                    || state.loading.get()
                                            })
                                    },
                                ))
                                .child(
                                    action("Save · ⌘S", Variant::Primary, move |_| save(ui))
                                        .disabled(move || {
                                            ui.is_method_pending("workflow.apply")
                                                || state.loading.get()
                                                || !apply.get()
                                        }),
                                ),
                        ),
                )))
        .child(when(move || state.agent.get(), move || agent_panel(ui)))
        .child(when(move || state.history_open.get(), move || history_panel(ui)))
}
fn source_dirty(draft: &Draft, name: &str) -> bool {
    let original = draft.base["sources"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["filename"] == name));
    original.is_none_or(|row| {
        row["source"].as_str() != draft.sources.get(name).map(String::as_str)
            || row["unpublished"] == true
    })
}
fn new_workflow(ui: Ui) {
    let state = ui.workflows;
    let project = ui.project.get_untracked();
    let mut name = String::new();
    state.drafts.update(|d| {
        let draft = d.entry(project).or_default();
        let mut n = 1;
        while draft.sources.contains_key(&format!("workflow-{n}.yaml")) { n += 1; }
        name = format!("workflow-{n}.yaml");
        draft.sources.insert(name.clone(), format!("schema: 1\nid: workflow-{}\nname: New workflow\non:\n  manual:\n    event: manual\n    start_at: explain\nsteps:\n  - id: explain\n    uses: agent\n    with:\n      profile: {}\n      instruction: |\n        Read the Card and propose a concrete plan.\n", canban_types::id(), ui.profiles.with_untracked(|p| p.first().map(|p| p.id.clone()).unwrap_or_default())));
        draft.selected = name.clone();
    });
    state.selected.set(name);
}
fn update_doc(ui: Ui, mutate: impl FnOnce(&mut canban_workflow_engine::source::Document)) {
    let state = ui.workflows;
    let project = ui.project.get_untracked();
    let name = state.selected.get_untracked();
    let Some(raw) = state
        .drafts
        .with_untracked(|d| d.get(&project).and_then(|d| d.sources.get(&name)).cloned())
    else {
        return;
    };
    let Ok(mut doc) = canban_workflow_engine::source::parse(&raw) else {
        return;
    };
    mutate(&mut doc);
    if let Ok(encoded) = canban_workflow_engine::source::encode(&doc) {
        state.drafts.update(|d| {
            if let Some(d) = d.get_mut(&project) {
                d.sources.insert(name, encoded);
            }
        });
    }
}

fn visual_studio(ui: Ui) -> AnyElement {
    use canban_workflow_engine::source::{
        self, default_step_prompt, is_default_step_prompt,
    };
    let state = ui.workflows;
    let name = state.selected.get();
    let project = ui.project.get();
    if name.is_empty() {
        return view()
            .flex_1()
            .items_center()
            .justify_center()
            .child(muted("Choose or create a workflow"))
            .into_element();
    }
    let raw = state
        .drafts
        .with(|d| d.get(&project).and_then(|d| d.sources.get(&name)).cloned())
        .unwrap_or_default();
    let doc = match source::parse(&raw) {
        Ok(doc) => doc,
        Err(err) => {
            return view()
                .flex_1()
                .min_h(px(0.))
                .gap_3()
                .child(notice(
                    format!("YAML syntax or schema error: {err}. Switch to YAML Source to fix it."),
                    Tone::Danger,
                ))
                .child(source_editor(ui))
                .into_element();
        }
    };
    let policy = source::detect_policy(&doc);
    let current_policy = policy.clone().unwrap_or_default();
    let is_custom = policy.is_none();

    let effective_columns = if !doc.columns.is_empty() {
        doc.columns.clone()
    } else {
        ui.current_project()
            .map(|p| {
                p.workflow
                    .stages
                    .iter()
                    .map(|s| canban_types::WorkflowColumn {
                        id: s.id.clone(),
                        name: s.name.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![
                    canban_types::WorkflowColumn {
                        id: "backlog".into(),
                        name: "Backlog".into(),
                    },
                    canban_types::WorkflowColumn {
                        id: "develop".into(),
                        name: "Develop".into(),
                    },
                    canban_types::WorkflowColumn {
                        id: "review".into(),
                        name: "Review".into(),
                    },
                ]
            })
    };

    let visible_steps: Vec<_> = doc
        .steps
        .iter()
        .filter(|s| s.uses != "complete")
        .cloned()
        .collect();
    let active_step_id = {
        let current = state.selected_step.get();
        if visible_steps.iter().any(|s| s.id == current) {
            current
        } else {
            visible_steps
                .iter()
                .find(|s| s.uses == "agent")
                .or_else(|| visible_steps.first())
                .map(|s| s.id.clone())
                .unwrap_or_default()
        }
    };

    // 1. Guided Agent Callout Banner
    let banner = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_3()
        .p_3()
        .rounded(px(RADIUS))
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.))
                .flex_1()
                .child(icon(Icon::Agent))
                .child(
                    view()
                        .gap_1()
                        .min_w(px(0.))
                        .flex_1()
                        .child(
                            label("Need a custom multi-column flow or special PR automation?")
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .child(muted(
                            "Use Edit with agent to let an Agent inspect your Project, guide you through workflow design, and validate & apply the YAML.",
                        )),
                ),
        )
        .child(compact_action(
            "Ask Agent to design…",
            Variant::Secondary,
            move |_| edit_agent(ui),
        ));

    // 2. Built-in Policy Bar
    let p_plan1 = current_policy.clone();
    let p_plan2 = current_policy.clone();
    let p_plan3 = current_policy.clone();
    let p_del1 = current_policy.clone();
    let p_del2 = current_policy.clone();
    let p_ext1 = current_policy.clone();
    let p_ext2 = current_policy.clone();
    let p_ext3 = current_policy.clone();

    let policy_section = panel()
        .gap_3()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .gap_1()
                        .child(label("Built-in Workflow Policies").font_weight(FontWeight::SEMIBOLD))
                        .child(muted(
                            "Choose high-level delivery & external PR rules without editing raw YAML. Per-step prompt customizations are preserved.",
                        )),
                )
                .child(status(
                    if is_custom {
                        "Custom Flow"
                    } else {
                        "Standard Policy"
                    },
                    if is_custom {
                        Tone::Attention
                    } else {
                        Tone::Info
                    },
                )),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap_4()
                .child({
                    let p_val = current_policy.planning.clone();
                    let p1 = p_val.clone();
                    let p2 = p_val.clone();
                    let p3 = p_val;
                    view()
                        .gap_1()
                        .child(label("1. Planning (Backlog)"))
                        .child(
                            segmented()
                                .child(compact_segment(
                                    "Manual",
                                    None,
                                    move || !is_custom && p1 == "manual",
                                    move |_| {
                                        let mut next = p_plan1.clone();
                                        next.planning = "manual".into();
                                        update_doc(ui, |d| source::apply_policy(d, &next));
                                    },
                                ))
                                .child(compact_segment(
                                    "Auto-plan (Confirm)",
                                    None,
                                    move || !is_custom && p2 == "confirm",
                                    move |_| {
                                        let mut next = p_plan2.clone();
                                        next.planning = "confirm".into();
                                        update_doc(ui, |d| source::apply_policy(d, &next));
                                    },
                                ))
                                .child(compact_segment(
                                    "Auto-plan & Develop",
                                    None,
                                    move || !is_custom && p3 == "auto",
                                    move |_| {
                                        let mut next = p_plan3.clone();
                                        next.planning = "auto".into();
                                        update_doc(ui, |d| source::apply_policy(d, &next));
                                    },
                                )),
                        )
                })
                .child({
                    let d_val = current_policy.delivery.clone();
                    let d1 = d_val.clone();
                    let d2 = d_val;
                    view()
                        .gap_1()
                        .child(label("2. Review & Delivery"))
                        .child(
                            segmented()
                                .child(compact_segment(
                                    "Human Review",
                                    None,
                                    move || !is_custom && d1 == "human_review",
                                    move |_| {
                                        let mut next = p_del1.clone();
                                        next.delivery = "human_review".into();
                                        update_doc(ui, |d| source::apply_policy(d, &next));
                                    },
                                ))
                                .child(compact_segment(
                                    "Auto-merge Queue",
                                    None,
                                    move || !is_custom && d2 == "auto_merge",
                                    move |_| {
                                        let mut next = p_del2.clone();
                                        next.delivery = "auto_merge".into();
                                        update_doc(ui, |d| source::apply_policy(d, &next));
                                    },
                                )),
                        )
                })
                .child({
                    let e_val = current_policy.external_pr.clone();
                    let e1 = e_val.clone();
                    let e2 = e_val.clone();
                    let e3 = e_val;
                    view()
                        .gap_1()
                        .child(label("3. External PR Rule"))
                        .child(
                            segmented()
                                .child(compact_segment(
                                    "Require Human Review",
                                    None,
                                    move || !is_custom && e1 == "require_review",
                                    move |_| {
                                        let mut next = p_ext1.clone();
                                        next.external_pr = "require_review".into();
                                        update_doc(ui, |d| source::apply_policy(d, &next));
                                    },
                                ))
                                .child(compact_segment(
                                    "Same as Internal",
                                    None,
                                    move || !is_custom && e2 == "same",
                                    move |_| {
                                        let mut next = p_ext2.clone();
                                        next.external_pr = "same".into();
                                        update_doc(ui, |d| source::apply_policy(d, &next));
                                    },
                                ))
                                .child(compact_segment(
                                    "Manual Only",
                                    None,
                                    move || !is_custom && e3 == "manual_only",
                                    move |_| {
                                        let mut next = p_ext3.clone();
                                        next.external_pr = "manual_only".into();
                                        update_doc(ui, |d| source::apply_policy(d, &next));
                                    },
                                )),
                        )
                }),
        );

    // 3. Visual Pipeline & Board Columns
    let mut columns_row = view().flex_row().flex_wrap().items_center().gap_2();
    let cols_snapshot = effective_columns.clone();
    for (idx, col) in effective_columns.iter().enumerate() {
        if idx > 0 {
            columns_row = columns_row.child(muted("→"));
        }
        let col_id = col.id.clone();
        let col_name = col.name.clone();
        let can_remove = effective_columns.len() > 1
            && !matches!(col_id.as_str(), "backlog" | "develop" | "review");
        let cols_for_remove = cols_snapshot.clone();
        let mut chip = view()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .bg(INSET)
            .border_1()
            .border_color(BORDER)
            .child(label(format!("{} ({})", col_name, col_id)));
        if can_remove {
            let remove_id = col_id.clone();
            chip = chip.child(compact_action("×", Variant::Quiet, move |_| {
                let remaining: Vec<_> = cols_for_remove
                    .iter()
                    .filter(|c| c.id != remove_id)
                    .cloned()
                    .collect();
                update_doc(ui, |d| {
                    d.columns = remaining;
                });
            }));
        }
        columns_row = columns_row.child(chip);
    }
    let cols_for_add = cols_snapshot;
    let add_col_row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(columns_row)
        .child(
            field(state.new_column_name, "Add column (e.g. QA)…")
                .w(px(180.)),
        )
        .child(compact_action("+ Add column", Variant::Secondary, move |_| {
            let raw_name = state.new_column_name.get_untracked();
            let title = raw_name.trim().to_owned();
            if title.is_empty() {
                return;
            }
            let slug: String = title
                .to_ascii_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect::<String>()
                .trim_matches('_')
                .to_owned();
            if slug.is_empty() || cols_for_add.iter().any(|c| c.id == slug) {
                return;
            }
            let mut updated_cols = cols_for_add.clone();
            // Insert before review if review exists, else at the end
            if let Some(pos) = updated_cols.iter().position(|c| c.id == "review") {
                updated_cols.insert(
                    pos,
                    canban_types::WorkflowColumn {
                        id: slug,
                        name: title,
                    },
                );
            } else {
                updated_cols.push(canban_types::WorkflowColumn {
                    id: slug,
                    name: title,
                });
            }
            state.new_column_name.set(String::new());
            update_doc(ui, |d| {
                d.columns = updated_cols;
            });
        }));

    let mut steps_flow = view().flex_row().flex_wrap().items_center().gap_2();
    for (idx, step) in visible_steps.iter().enumerate() {
        if idx > 0 {
            steps_flow = steps_flow.child(muted("→"));
        }
        let step_id = step.id.clone();
        let selected_id = active_step_id.clone();
        let title = step.name.clone().unwrap_or_else(|| step.id.clone());
        let subtitle = match step.uses.as_str() {
            "agent" => {
                let prof = step
                    .with
                    .get("profile")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let instr = step
                    .with
                    .get("instruction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let customized = !is_default_step_prompt(&step.id, Some(prof), instr);
                format!(
                    "@{} · {}",
                    prof,
                    if customized {
                        "Custom prompt"
                    } else {
                        "Default prompt"
                    }
                )
            }
            "card.move" => format!(
                "Move → {}",
                step.with
                    .get("column")
                    .and_then(|v| v.as_str())
                    .unwrap_or("column")
            ),
            "approval" => "Human approval gate".into(),
            "deliver" | "card.deliver" => "Auto-merge queue (unless held)".into(),
            "condition" => format!(
                "If {} == {}",
                step.with
                    .get("pointer")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
                step.with.get("equals").unwrap_or(&Value::Null)
            ),
            other => other.to_owned(),
        };
        let is_selected = step_id == selected_id;
        let click_id = step_id.clone();
        steps_flow = steps_flow.child(
            nsh_base::button()
                .p_3()
                .gap_1()
                .rounded(px(RADIUS))
                .bg(if is_selected { SELECTED } else { INSET })
                .border_1()
                .border_color(if is_selected { ACCENT } else { BORDER })
                .hover(|s| s.border_color(ACCENT))
                .on_press(move |_: &()| state.selected_step.set(click_id.clone()))
                .child(label(title).font_weight(FontWeight::SEMIBOLD))
                .child(muted(subtitle)),
        );
    }

    let pipeline_section = panel()
        .gap_3()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .gap_1()
                .child(label("Board Columns & Visual Pipeline").font_weight(FontWeight::SEMIBOLD))
                .child(muted(
                    "Define 3, 4, or more Board columns and click any step below to customize its Agent Profile or Prompt.",
                )),
        )
        .child(add_col_row)
        .child(steps_flow);

    // 4. Step Prompt & Profile Inspector (in the same window!)
    let inspector = if let Some(step) = visible_steps.iter().find(|s| s.id == active_step_id) {
        let step_id = step.id.clone();
        let step_title = step.name.clone().unwrap_or_else(|| step.id.clone());
        match step.uses.as_str() {
            "agent" => {
                let current_profile = step
                    .with
                    .get("profile")
                    .and_then(|v| v.as_str())
                    .unwrap_or("developer")
                    .to_owned();
                let current_instruction = step
                    .with
                    .get("instruction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let default_prompt = default_step_prompt(&step_id, Some(&current_profile));
                let is_default =
                    is_default_step_prompt(&step_id, Some(&current_profile), &current_instruction);

                let prompt_sig = signal(current_instruction.clone());
                let step_for_effect = step_id.clone();
                effect(move || {
                    let val = prompt_sig.get();
                    if val != current_instruction {
                        let target_id = step_for_effect.clone();
                        update_doc(ui, |d| {
                            if let Some(s) = d.steps.iter_mut().find(|s| s.id == target_id) {
                                s.with.insert("instruction".into(), json!(val));
                            }
                        });
                    }
                });

                let step_for_reset = step_id.clone();
                let step_for_prof = step_id.clone();
                let active_prof = current_profile.clone();
                let prompt_editor = canban_ui_kit::markdown::MarkdownInput::new();

                panel()
                    .gap_3()
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
                                    .child(
                                        label(format!("Step Prompt & Agent · {step_title} ({step_id})"))
                                            .font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(status(
                                        if is_default {
                                            "Default prompt"
                                        } else {
                                            "Customized prompt"
                                        },
                                        if is_default {
                                            Tone::Neutral
                                        } else {
                                            Tone::Info
                                        },
                                    )),
                            )
                            .child(when(
                                move || !is_default,
                                move || {
                                    let reset_id = step_for_reset.clone();
                                    compact_action(
                                        "Reset to default prompt",
                                        Variant::Secondary,
                                        move |_| {
                                            let def =
                                                default_step_prompt(&reset_id, None).to_owned();
                                            prompt_sig.set(def.clone());
                                            let target = reset_id.clone();
                                            update_doc(ui, |d| {
                                                if let Some(s) =
                                                    d.steps.iter_mut().find(|s| s.id == target)
                                                {
                                                    s.with.insert("instruction".into(), json!(def));
                                                }
                                            });
                                        },
                                    )
                                },
                            )),
                    )
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(muted("Agent Profile:"))
                            .child(segmented().child(each(
                                move || ui.profiles.get(),
                                |p| p.id.clone(),
                                move |p, _| {
                                    let p = p.get_untracked();
                                    let pid = p.id.clone();
                                    let is_prof = pid == active_prof;
                                    let target = step_for_prof.clone();
                                    compact_segment(
                                        format!("@{pid}"),
                                        None,
                                        move || is_prof,
                                        move |_| {
                                            let chosen = pid.clone();
                                            let tid = target.clone();
                                            update_doc(ui, |d| {
                                                if let Some(s) =
                                                    d.steps.iter_mut().find(|s| s.id == tid)
                                                {
                                                    s.with.insert("profile".into(), json!(chosen));
                                                }
                                            });
                                        },
                                    )
                                },
                            ))),
                    )
                    .child(
                        view()
                            .h(px(180.))
                            .bg(INSET)
                            .rounded(px(RADIUS))
                            .p_2()
                            .child(
                                prompt_editor
                                    .source(
                                        prompt_sig,
                                        "Step Prompt",
                                        default_prompt,
                                    )
                                    .size_full(),
                            ),
                    )
                    .into_element()
            }
            "deliver" | "card.deliver" => panel()
                .gap_2()
                .border_1()
                .border_color(BORDER)
                .child(
                    label(format!("Auto-merge Step · {step_title} ({step_id})"))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(muted(
                    "Automatically previews and merges the Card worktree into the target branch after verification succeeds. Users can click 'Hold auto-merge' on any Card to pause this step for manual approval.",
                ))
                .into_element(),
            "approval" => panel()
                .gap_2()
                .border_1()
                .border_color(BORDER)
                .child(
                    label(format!("Human Approval Gate · {step_title} ({step_id})"))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(muted(
                    step.with
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Pauses workflow execution until a human approves or requests changes.")
                        .to_owned(),
                ))
                .into_element(),
            _ => panel()
                .gap_2()
                .border_1()
                .border_color(BORDER)
                .child(
                    label(format!("Step · {step_title} ({step_id})"))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(muted(
                    "Switch to YAML Source to edit low-level step transition arguments.",
                ))
                .into_element(),
        }
    } else {
        view().into_element()
    };

    scroll()
        .flex_1()
        .min_h(px(0.))
        .child(
            view()
                .gap_3()
                .pb_4()
                .child(banner)
                .child(policy_section)
                .child(pipeline_section)
                .child(inspector),
        )
        .into_element()
}

fn source_editor(ui: Ui) -> AnyElement {
    let state = ui.workflows;
    let name = state.selected.get();
    let project = ui.project.get_untracked();
    if name.is_empty() {
        return view()
            .flex_1()
            .items_center()
            .justify_center()
            .child(muted("Choose or create a workflow"))
            .into_element();
    }
    let initial = state
        .drafts
        .with_untracked(|d| d.get(&project).and_then(|d| d.sources.get(&name)).cloned())
        .unwrap_or_default();
    let text = signal(initial);
    let editor = canban_ui_kit::markdown::MarkdownInput::new();
    editor.restore_after(
        computed(move || state.agent.get() || state.history_open.get() || ui.quick_open.get()),
        computed(move || ui.page.get() == "workflows"),
    );
    let sync_name = name.clone();
    let sync_project = project.clone();
    effect(move || {
        let saved = state.drafts.with(|d| {
            d.get(&sync_project)
                .and_then(|d| d.sources.get(&sync_name))
                .cloned()
        });
        if let Some(saved) = saved
            && saved != text.get_untracked()
        {
            text.set(saved);
        }
    });
    effect(move || {
        let value = text.get();
        state.drafts.update(|d| {
            if let Some(d) = d.get_mut(&project) {
                d.sources.insert(name.clone(), value);
            }
        });
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .bg(SURFACE)
        .rounded(px(RADIUS))
        .p_2()
        .child(
            editor
                .source(text, "Workflow YAML", "Write Workflow YAML…")
                .language(Some("yaml".into()))
                .line_numbers(true)
                .indent_guides(true)
                .auto_indent(true)
                .auto_close(true)
                .tab_size(Some(2)),
        )
        .into_element()
}
/// Real native modal surface: editor layers behind it cannot paint through it.
fn workflow_dialog(
    content: impl IntoElement,
    close: impl Fn() + Copy + 'static,
) -> impl IntoElement {
    // A native editor can blur on Escape before the modal host receives cancel.
    // Keep dismissal scoped to this mounted dialog, including its editor sink.
    on_key_with(
        "escape",
        KeyOptions {
            when: nsh_core::keymap::When::Always,
            ..Default::default()
        },
        move |_| close(),
    );
    nsh_base::dialog()
        .animation(canban_ui_kit::motion::modal)
        .close_on_escape(true)
        .close_on_backdrop_press(true)
        .on_cancel(close)
        .child(
            view()
                .flex_1()
                .min_h(px(0.))
                .p_5()
                .bg(rgba(0x00000066))
                .items_center()
                .justify_center()
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .on_press(close)
                        .label("Dismiss workflow dialog"),
                )
                .child(
                    view()
                        .w_full()
                        .max_w(px(1040.))
                        .flex_1()
                        .min_h(px(0.))
                        .max_h(px(780.))
                        .p_5()
                        .gap_3()
                        .bg(SURFACE)
                        .border_1()
                        .border_color(BORDER)
                        .rounded(px(RADIUS))
                        .child(content),
                ),
        )
}
/// Shared production panel, also exercised in the native Gallery.
pub fn agent_panel(ui: Ui) -> impl IntoElement {
    let state = ui.workflows;
    let prompt_editor = component_ref::<nsh_base::Editor>();
    spawn(async move {
        next_frame().await;
        if state.agent.try_get() == Some(true)
            && let Some(api) = prompt_editor.api::<nsh_base::EditorApi>()
        {
            api.focus();
        }
    });
    workflow_dialog(view().flex_1().min_h(px(0.)).gap_3()
        .child(view().flex_row().items_center().gap_3().child(icon(Icon::Agent)).child(heading("Edit with agent")).child(view().flex_1()).child(chrome_button(Icon::Close,"Close agent editor",move|_|state.agent.set(false))))
        .child(muted("Review this Project-wide Prompt, then copy it or start a configuration conversation in Terminal."))
        .child(view().flex_row().flex_wrap().gap_2().child(each(move||ui.profiles.get(),|p|p.id.clone(),move|p,_|{let p=p.get_untracked();let id=p.id.clone();let selected=p.id.clone();action(format!("@{}",p.id),Variant::Secondary,move|_|state.profile.set(id.clone())).bg(move||if state.profile.get()==selected{SELECTED}else{INSET})})))
        .child(view().flex_1().min_h(px(0.)).bg(INSET).p_2().child(nsh_base::editor().value(move||Some(state.prompt.get().into())).read_only(true).soft_wrap(true).line_numbers(false).component_ref(&prompt_editor).accessibility_label("Project Workflow configuration Prompt").size_full()))
        .child(when(move || !state.error.get().is_empty(), move || notice(move || state.error.get(), Tone::Attention)))
        .child(muted(move||state.copy_status.get()))
        .child(view().flex_row().flex_wrap().gap_2().child(action("Select all",Variant::Secondary,move|_|{if let Some(api)=prompt_editor.api::<nsh_base::EditorApi>(){api.focus();api.run(nsh_base::EditorCommand::SelectAll);}})).child(action("Copy prompt",Variant::Secondary,move|_|copy_prompt(ui))).child(view().flex_1()).child(action(move||{
            let key=format!("configuration:{}:workflows:{}",ui.project.get(),state.profile.get());ui.snapshot.with(|s|{if let Some(c)=s.configuration_sessions.iter().find(|c|c.id==key){if c.state=="finished"{"Resume and open Terminal"}else{"Continue in Terminal"}}else{"Start and open Terminal"}})
        },Variant::Primary,move|_|launch(ui,false)).disabled(move||ui.pending.get()||state.profile.get().is_empty())).child(action("New conversation",Variant::Quiet,move|_|launch(ui,true)).disabled(move||ui.pending.get()||state.profile.get().is_empty()))), move || state.agent.set(false))
}
/// Pure active-definition projection; no source text or prose RPC is needed.
pub fn column_trigger(ui: Ui, column: String) -> impl IntoElement {
    let summary_col = column.clone();
    let summary = computed(move || {
        let project = ui.project.get();
        ui.snapshot.with(|s| {
            let files = s
                .workflow_sources
                .iter()
                .filter(|f| f.project_id == project)
                .cloned()
                .collect::<Vec<_>>();
            canban_workflow_engine::source::column_summary(
                &summary_col,
                &files,
                &s.workflow_revisions,
            )
        })
    });
    let details = computed(move || {
        let project = ui.project.get();
        ui.snapshot.with(|s| {
            let files = s
                .workflow_sources
                .iter()
                .filter(|f| f.project_id == project)
                .cloned()
                .collect::<Vec<_>>();
            canban_workflow_engine::source::column_details(
                &column,
                &files,
                &s.workflow_revisions,
                &s.event_subscriptions,
            )
        })
    });
    let open = signal(false);
    when(
        move || !details.get().is_empty(),
        move || {
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(when(
                    move || summary.get().is_some(),
                    move || badge(move || summary.get().unwrap_or_default()),
                ))
                .child(
                    nsh_base::popover()
                        .open(move || Some(open.get()))
                        .on_open_change(move |event: &nsh_base::PopoverEvent| {
                            if let nsh_base::PopoverEvent::OpenChange(value, _) = event {
                                open.set(*value);
                            }
                        })
                        .placement(Placement::Bottom)
                        .align(Align::End)
                        .offset(px(8.))
                        .margin(px(12.))
                        .trigger(
                            chrome_action(Icon::Trigger, "Column automations", |_| {})
                                .selected(move || open.get())
                                .bg(move || if open.get() { SELECTED } else { CLEAR }),
                        )
                        .child(
                            view()
                                .w(px(360.))
                                .max_w(Percentage(0.9))
                                .p_4()
                                .gap_3()
                                .bg(SURFACE)
                                .border_1()
                                .border_color(BORDER)
                                .rounded(px(RADIUS))
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(icon(Icon::Trigger))
                                        .child(label("Automations").flex_1())
                                        .child(chrome_action(
                                            Icon::Close,
                                            "Close automations",
                                            move |_| open.set(false),
                                        )),
                                )
                                .child(scroll().max_h(px(360.)).child(view().gap_3().child(each(
                                    move || details.get(),
                                    |detail| detail.clone(),
                                    move |detail, _| {
                                        read_text(detail.get_untracked())
                                            .text_size(px(12.))
                                            .text_color(TEXT)
                                    },
                                ))))
                                .child(
                                    action_with_icon(
                                        Icon::Trigger,
                                        "View workflows",
                                        Variant::Secondary,
                                        move |_| {
                                            open.set(false);
                                            ui.navigate("workflows");
                                        },
                                    )
                                    .w_full(),
                                ),
                        ),
                )
        },
    )
}

fn show_history(ui: Ui) {
    let origin = ui.location();
    let state = ui.workflows;
    let project = ui.project.get_untracked();
    let name = state.selected.get_untracked();
    let workflow = state.drafts.with_untracked(|d| {
        d.get(&project)
            .and_then(|d| d.base["sources"].as_array())
            .and_then(|rows| rows.iter().find(|r| r["filename"] == name))
            .and_then(|r| r["active"]["workflow_id"].as_str())
            .map(str::to_owned)
    });
    let Some(workflow) = workflow else {
        state
            .error
            .set("Save this Workflow before viewing its history.".into());
        return;
    };
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    ui.spawn(async move {
        let result = blocking(move || {
            app.call(
                "workflow.list",
                json!({"project_id":project,"workflow_id":workflow}),
            )
        })
        .await;
        if ui.location() != origin {
            return;
        }
        match result {
            Ok(Ok(v)) => match serde_json::from_value::<Vec<canban_types::WorkflowRevision>>(v) {
                Ok(mut history) => {
                    history.retain(|w| w.source_yaml.is_some());
                    history.sort_by_key(|w| std::cmp::Reverse(w.revision));
                    state
                        .history_revision
                        .set(history.first().map_or(0, |w| w.revision));
                    state.history.set(history);
                    state.history_name.set(name);
                    state.history_open.set(true);
                }
                Err(e) => state.error.set(e.to_string()),
            },
            Ok(Err(e)) => state.error.set(format!("{e:#}")),
            Err(e) => state.error.set(e.to_string()),
        }
    });
}
fn history_panel(ui: Ui) -> impl IntoElement {
    let state = ui.workflows;
    let editor = component_ref::<nsh_base::Editor>();
    spawn(async move {
        next_frame().await;
        if state.history_open.try_get() == Some(true)
            && let Some(api) = editor.api::<nsh_base::EditorApi>()
        {
            api.focus();
        }
    });
    workflow_dialog(view().flex_1().min_h(px(0.)).gap_3()
        .child(view().flex_row().items_center().gap_2().child(heading(move||format!("{} / History",state.history_name.get()))).child(view().flex_1()).child(chrome_button(Icon::Close,"Close workflow history",move|_|state.history_open.set(false))))
        .child(muted("Published SQL revisions are immutable. Loading an earlier version creates an editor draft; Save publishes it as a new version."))
        .child(view().flex_1().min_h(px(0.)).flex_row().gap_3().child(scroll().w(px(190.)).child(each(move||state.history.get(),|w|w.id.clone(),move|w,_|{let revision=w.get_untracked().revision;action(format!("Version {revision}"),Variant::Secondary,move|_|state.history_revision.set(revision)).bg(move||if state.history_revision.get()==revision{SELECTED}else{INSET})})))
            .child(view().flex_1().min_h(px(0.)).child(nsh_base::editor().value(move||Some(state.history.with(|h|h.iter().find(|w|w.revision==state.history_revision.get()).and_then(|w|w.source_yaml.clone()).unwrap_or_default()).into())).language(Some("yaml".into())).read_only(true).soft_wrap(true).line_numbers(true).indent_guides(true).folding(false).component_ref(&editor).accessibility_label("Historical Workflow YAML").size_full())))
        .child(view().flex_row().justify_end().child(action("Load as draft",Variant::Primary,move|_|{
            if let Some(raw)=state.history.with_untracked(|h|h.iter().find(|w|w.revision==state.history_revision.get_untracked()).and_then(|w|w.source_yaml.clone())){
                let name=state.history_name.get_untracked();let project=ui.project.get_untracked();state.drafts.update(|d|{if let Some(d)=d.get_mut(&project){d.sources.insert(name,raw);}});state.history_open.set(false);
            }
        }))), move || state.history_open.set(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_receipt_retains_text_entered_after_submit() {
        let mut draft = Draft::default();
        draft.accept(json!({"revision":"old","sources":[{"filename":"test.yaml","source":"old"}]}));
        draft.sources.insert("test.yaml".into(), "submitted".into());
        let submitted = draft.sources.clone();
        draft
            .sources
            .insert("test.yaml".into(), "new keystrokes".into());
        draft.accept_publication(
            json!({"revision":"new","sources":[{"filename":"test.yaml","source":"submitted"}]}),
            &submitted,
        );
        assert_eq!(draft.sources["test.yaml"], "new keystrokes");
        assert_eq!(draft.base["revision"], "new");
        assert!(draft.dirty());
    }
}
