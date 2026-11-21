//! Project Workflows owns source drafts. Source editors and Prompt preview use
//! native NSH selection/editing; launching is a separate explicit intent.
pub use crate::schedules::{
    add_schedule_rule_to_file, collect_project_schedules, new_scheduled_workflow, open_schedules,
    project_schedule_count, toggle_schedule_by_id, trigger_schedule_by_id,
};
mod studio;
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
    pub(crate) fn dirty(&self) -> bool {
        self.inputs()
            != self.base["sources"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|v| json!({"filename":v["filename"],"source":v["source"]}))
                .collect::<Vec<_>>()
    }
    pub(crate) fn needs_apply(&self) -> bool {
        self.dirty()
            || self.base["sources"]
                .as_array()
                .is_some_and(|files| files.iter().any(|f| f["unpublished"] == true))
    }
    pub(crate) fn inputs(&self) -> Vec<Value> {
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
    pub(crate) fn accept(&mut self, value: Value) {
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
type SourceStamp = Vec<(String, u64, String)>;
#[derive(Clone, Copy)]
pub struct State {
    pub drafts: Signal<BTreeMap<String, Draft>>,
    pub selected: Signal<String>,
    pub section: Signal<String>,
    pub selected_schedule: Signal<String>,
    pub schedule_artifact: Signal<Option<(String, String)>>,
    pub mode: Signal<String>,
    pub selected_step: Signal<String>,
    pub new_column_name: Signal<String>,
    pub studio_doc: Signal<Option<canban_workflow_engine::source::Document>>,
    pub studio_error: Signal<String>,
    pub(crate) studio_synced: Stored<(String, String, String)>,
    stamps: Stored<BTreeMap<String, SourceStamp>>,
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
            section: signal("workflows".into()),
            selected_schedule: signal(String::new()),
            schedule_artifact: signal(None),
            mode: signal("visual".into()),
            selected_step: signal(String::new()),
            new_column_name: signal(String::new()),
            studio_doc: signal(None),
            studio_error: signal(String::new()),
            studio_synced: stored((String::new(), String::new(), String::new())),
            stamps: stored(BTreeMap::new()),
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
pub fn chrome_bar(ui: Ui) -> impl IntoElement {
    let state = ui.workflows;
    let sched_count = computed(move || crate::schedules::project_schedule_count(ui));
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
                .child(compact_action("← Board", Variant::Quiet, move |_| {
                    ui.navigate("board")
                }))
                .child(
                    label(move || {
                        format!(
                            "{} / Workflows",
                            ui.current_project().map(|p| p.name).unwrap_or_default()
                        )
                    })
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(13.5))
                    .truncate()
                    .min_w(px(0.)),
                ),
        )
        .child(view().flex_1().h_full().window_drag(true))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .window_drag(false)
                .child(compact_action(
                    move || format!("Schedules ({})", sched_count.get()),
                    Variant::Quiet,
                    move |_| {
                        crate::schedules::open(ui, Some(ui.project.get_untracked()), None);
                    },
                ))
                .child(compact_action(
                    "Project Agents",
                    Variant::Quiet,
                    move |_| ui.navigate("project-agents"),
                ))
                .child(
                    compact_action("Edit with agent", Variant::Primary, move |_| edit_agent(ui))
                        .disabled(move || ui.pending.get() || state.loading.get()),
                ),
        )
}

pub(crate) fn project_template_files_from_snapshot(
    snapshot: &canban_types::Snapshot,
    project_id: &str,
) -> Vec<canban_types::WorkflowTemplateFile> {
    let mut active: Vec<_> = snapshot
        .workflow_sources
        .iter()
        .filter(|s| s.project_id == project_id)
        .collect();
    active.sort_by(|a, b| a.filename.cmp(&b.filename));
    active
        .into_iter()
        .filter_map(|item| {
            let raw = snapshot
                .workflow_revisions
                .iter()
                .find(|r| r.id == item.revision_id)
                .and_then(|r| r.source_yaml.clone())?;
            Some(canban_types::WorkflowTemplateFile {
                filename: item.filename.clone(),
                source: raw,
            })
        })
        .collect()
}

pub fn set_default_workflow(ui: Ui) {
    let state = ui.workflows;
    let project = ui.project.get_untracked();
    if project.is_empty() {
        ui.notify("Select or create a Project first");
        return;
    }
    let has_unsaved = state
        .drafts
        .with_untracked(|d| d.get(&project).is_some_and(Draft::needs_apply));
    if has_unsaved {
        ui.notify(format!(
            "Save workflow changes first ({}) before setting as default",
            crate::commands::shortcuts::SAVE_WORKFLOW_DISPLAY
        ));
        return;
    }
    if !crate::backend::is_connected() {
        let project_name = ui.current_project().map(|p| p.name);
        let draft_files: Vec<canban_types::WorkflowTemplateFile> = state
            .drafts
            .with_untracked(|d| {
                d.get(&project).map(|dr| {
                    dr.sources
                        .iter()
                        .map(|(k, v)| canban_types::WorkflowTemplateFile {
                            filename: k.clone(),
                            source: v.clone(),
                        })
                        .collect::<Vec<_>>()
                })
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| {
                ui.snapshot
                    .with_untracked(|s| project_template_files_from_snapshot(s, &project))
            });
        if draft_files.is_empty() {
            return;
        }
        ui.snapshot.update(|s| {
            s.workflow_templates
                .retain(|t| t.id != canban_types::DEFAULT_WORKFLOW_TEMPLATE_ID);
            s.workflow_templates.push(canban_types::WorkflowTemplate {
                id: canban_types::DEFAULT_WORKFLOW_TEMPLATE_ID.into(),
                name: "Default".into(),
                source_project_name: project_name,
                sources: draft_files,
                updated_at: canban_types::now(),
            });
        });
        ui.notify("Saved as default workflow for new Projects");
        return;
    }
    let is_remote_project = ui
        .snapshot
        .with_untracked(|s| s.host_of(&project) != "local");
    if is_remote_project {
        let sources = ui
            .snapshot
            .with_untracked(|s| project_template_files_from_snapshot(s, &project));
        let source_name = ui.snapshot.with_untracked(|s| {
            s.projects
                .iter()
                .find(|p| p.id == project)
                .map(|p| p.name.clone())
        });
        dispatch(
            ui,
            "workflow.default.set",
            json!({
                "sources": sources,
                "source_project_name": source_name,
            }),
        );
    } else {
        dispatch(ui, "workflow.default.set", json!({"project_id": project}));
    }
}

pub fn reset_default_workflow(ui: Ui) {
    if !crate::backend::is_connected() {
        ui.snapshot.update(|s| {
            s.workflow_templates
                .retain(|t| t.id != canban_types::DEFAULT_WORKFLOW_TEMPLATE_ID);
        });
        ui.notify("Reset new Project workflow to built-in default");
        return;
    }
    dispatch(ui, "workflow.default.reset", json!({}));
}

fn project_source_stamp(ui: Ui, project: &str) -> SourceStamp {
    ui.snapshot.with_untracked(|s| {
        s.workflow_sources
            .iter()
            .filter(|f| f.project_id == project)
            .map(|f| (f.id.clone(), f.revision, f.hash.clone()))
            .collect()
    })
}
pub(crate) fn load(ui: Ui, discard: bool) {
    if !crate::backend::is_connected() {
        return;
    }
    let project = ui.project.get_untracked();
    let state = ui.workflows;
    let stamp = project_source_stamp(ui, &project);
    let has_draft = state.drafts.with_untracked(|d| d.contains_key(&project));
    if !discard
        && has_draft
        && state.stamps.with(|m| m.get(&project) == Some(&stamp))
        && state.latest.with_untracked(Value::is_null)
    {
        return;
    }
    let ticket = state.epoch.get_untracked() + 1;
    state.epoch.set(ticket);
    if discard || !has_draft {
        state.loading.set(true);
    }
    ui.spawn(async move {
        let id = project.clone();
        let result = blocking(move || {
            crate::routed_call("workflow.sources", json!({"project_id":id}), None)
        })
        .await;
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
                let latest_stamp = project_source_stamp(ui, &project);
                state.stamps.with(|m| {
                    m.insert(project.clone(), latest_stamp);
                });
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
pub(crate) fn save(ui: Ui) {
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
    if !crate::backend::is_connected() {
        for (fname, raw) in &draft.sources {
            match canban_workflow_engine::source::parse(raw) {
                Ok(doc) => match doc.compile() {
                    Ok(flow) => {
                        let cols = ui.snapshot.with_untracked(|s| {
                            s.projects
                                .iter()
                                .find(|p| p.id == project)
                                .map(|p| {
                                    p.workflow
                                        .stages
                                        .iter()
                                        .map(|st| st.id.clone())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                        });
                        if let Err(err) =
                            canban_workflow_engine::source::validate_schedules(&doc, &flow, &cols)
                        {
                            state.error.set(format!("{fname}: {err:#}"));
                            return;
                        }
                    }
                    Err(err) => {
                        state.error.set(format!("{fname}: {err:#}"));
                        return;
                    }
                },
                Err(err) => {
                    state.error.set(format!("{fname}: {err:#}"));
                    return;
                }
            }
        }
        state.drafts.update(|d| {
            let current = d.entry(project.clone()).or_default();
            let v = json!({
                "revision": "gallery",
                "sources": draft.inputs(),
            });
            current.accept_publication(v, &draft.sources);
        });
        crate::schedules::offline_save_schedules(ui, &project);
        state.latest.set(Value::Null);
        ui.notify("Workflow saved");
        return;
    }
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
pub fn edit_agent(ui: Ui) {
    let state = ui.workflows;
    let profile = crate::sessions::default_profile(ui);
    state.profile.set(profile.clone());
    if !crate::backend::is_connected() {
        state.agent.set(true);
        return;
    }
    let origin = ui.location();
    let project = ui.project.get_untracked();
    if project.is_empty() {
        ui.navigate("new-project");
        return;
    }
    state.error.set(String::new());
    dispatch_complete(
        ui,
        "configuration.open",
        json!({
            "project_id": project,
            "profile_id": profile,
            "auto_fresh": true,
        }),
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
fn launch(ui: Ui, fresh: bool) {
    let origin = ui.location();
    let state = ui.workflows;
    let profile = crate::sessions::default_profile(ui);
    state.profile.set(profile.clone());
    state.error.set(String::new());
    let mut params = json!({
        "project_id": ui.project.get_untracked(),
        "profile_id": profile,
        "fresh": fresh,
        "auto_fresh": !fresh,
    });
    let prompt = state.prompt.get_untracked();
    if !prompt.is_empty() {
        params["prompt"] = json!(prompt);
    }
    dispatch_complete(ui, "configuration.open", params, move |result| {
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
    });
}
fn copy_prompt(ui: Ui) {
    let state = ui.workflows;
    let prompt = state.prompt.get_untracked();
    set_clipboard(&prompt);
    state.copy_status.set("Copied".into());
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
    effect(move || {
        let project = ui.project.get();
        let name = state.selected.get();
        let raw = state
            .drafts
            .with(|d| d.get(&project).and_then(|d| d.sources.get(&name)).cloned());
        if name.is_empty() {
            state
                .studio_synced
                .replace((project, String::new(), String::new()));
            state.studio_error.set(String::new());
            state.studio_doc.set(None);
            return;
        }
        let raw = raw.unwrap_or_default();
        if state
            .studio_synced
            .with(|s| s.0 == project && s.1 == name && s.2 == raw)
        {
            return;
        }
        state.studio_synced.replace((project, name, raw.clone()));
        match canban_workflow_engine::source::parse(&raw) {
            Ok(doc) => {
                state.studio_error.set(String::new());
                state.studio_doc.set(Some(doc));
            }
            Err(err) => {
                state.studio_error.set(err.to_string());
                state.studio_doc.set(None);
            }
        }
    });
    let show_yaml = computed(move || state.mode.get() == "yaml" || !state.latest.get().is_null());
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
    let default_state = computed(move || {
        let project = ui.project.get();
        let draft_files: Vec<canban_types::WorkflowTemplateFile> = state
            .drafts
            .with(|d| {
                d.get(&project).map(|dr| {
                    dr.sources
                        .iter()
                        .map(|(k, v)| canban_types::WorkflowTemplateFile {
                            filename: k.clone(),
                            source: v.clone(),
                        })
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        ui.snapshot.with(|s| {
            let tpl = s
                .workflow_templates
                .iter()
                .find(|t| t.id == canban_types::DEFAULT_WORKFLOW_TEMPLATE_ID);
            let Some(tpl) = tpl else {
                return (false, false);
            };
            let files = if !draft_files.is_empty() {
                draft_files.clone()
            } else {
                project_template_files_from_snapshot(s, &project)
            };
            let matches = match (
                canban_workflow_engine::source::normalized_collection_signature(&files),
                canban_workflow_engine::source::normalized_collection_signature(&tpl.sources),
            ) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            (true, matches)
        })
    });
    on_key_with(
        crate::commands::shortcuts::SAVE_WORKFLOW_KEY,
        KeyOptions {
            when: nsh_core::keymap::When::Always,
            ..Default::default()
        },
        move |_| {
            if !state.agent.get_untracked()
                && !state.history_open.get_untracked()
                && state.schedule_artifact.with_untracked(Option::is_none)
                && !ui.quick_open.get_untracked()
            {
                save(ui);
            }
        },
    );
    let save_bar = move || {
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
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(muted(move || {
                        if apply.get() {
                            format!(
                                "Unsaved workflow changes · Press Save or {} to apply",
                                crate::commands::shortcuts::SAVE_WORKFLOW_DISPLAY
                            )
                        } else if default_state.get().1 {
                            "All workflow changes saved · Default for new Projects".into()
                        } else {
                            "All workflow changes saved".into()
                        }
                    }))
                    .child(when(
                        move || !apply.get() && default_state.get().1,
                        move || {
                            view()
                                .tooltip(
                                    hint_tooltip(
                                        "Revert new Projects back to the built-in default workflow",
                                    )
                                    .placement(Placement::Top),
                                )
                                .child(
                                    compact_action(
                                        "Reset default",
                                        Variant::Quiet,
                                        move |_| reset_default_workflow(ui),
                                    )
                                    .disabled(move || {
                                        ui.is_method_pending("workflow.default.reset")
                                            || state.loading.get()
                                    }),
                                )
                        },
                    )),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(when(
                        move || !apply.get() && !default_state.get().1,
                        move || {
                            view()
                                .tooltip(
                                    hint_tooltip(
                                        "Use this Project's saved workflows as the default for all new Projects",
                                    )
                                    .placement(Placement::Top),
                                )
                                .child(
                                    action(
                                        "Set as default",
                                        Variant::Secondary,
                                        move |_| set_default_workflow(ui),
                                    )
                                    .disabled(move || {
                                        ui.is_method_pending("workflow.default.set")
                                            || state.loading.get()
                                            || ui.project.get().is_empty()
                                    }),
                                )
                        },
                    ))
                    .child(when(
                        move || dirty.get() || !state.latest.get().is_null(),
                        move || {
                            action("Discard", Variant::Quiet, move |_| load(ui, true)).disabled(
                                move || {
                                    ui.is_method_pending("workflow.apply") || state.loading.get()
                                },
                            )
                        },
                    ))
                    .child(
                        action(
                            format!(
                                "Save · {}",
                                crate::commands::shortcuts::SAVE_WORKFLOW_DISPLAY
                            ),
                            Variant::Primary,
                            move |_| save(ui),
                        )
                        .disabled(move || {
                            ui.is_method_pending("workflow.apply")
                                || state.loading.get()
                                || !apply.get()
                        }),
                    ),
            )
    };
    view().flex_1().min_h(px(0.)).min_w(px(0.)).p_5().gap_4()
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
                .child(action_with_icon(Icon::Plus, "New workflow", Variant::Quiet, move |_| new_workflow(ui)).w_full().justify_start().disabled(move || ui.pending.get() || state.loading.get()))
                .child(action_with_icon(Icon::Clock, "New schedule", Variant::Quiet, move |_| new_scheduled_workflow(ui)).w_full().justify_start().disabled(move || ui.pending.get() || state.loading.get())))
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
                    if show_yaml.get() {
                        source_editor(ui)
                    } else {
                        untracked(|| visual_studio(ui))
                    }
                }))
                .child(save_bar())))
        .child(when(move || state.agent.get(), move || agent_panel(ui)))
        .child(when(move || state.history_open.get(), move || history_panel(ui)))
}
pub(crate) fn source_dirty(draft: &Draft, name: &str) -> bool {
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
        draft.sources.insert(name.clone(), format!("schema: {}\nid: workflow-{}\nname: New workflow\non:\n  manual:\n    event: manual\n    start_at: explain\nsteps:\n  - id: explain\n    uses: agent\n    with:\n      profile: {}\n      instruction: |\n        Read the Card and propose a concrete plan.\n", canban_workflow_engine::migrations::CURRENT, canban_types::id(), ui.profiles.with_untracked(|p| p.first().map(|p| p.id.clone()).unwrap_or_default())));
        draft.selected = name.clone();
    });
    state.selected.set(name);
}
fn update_doc(ui: Ui, mutate: impl FnOnce(&mut canban_workflow_engine::source::Document)) {
    let state = ui.workflows;
    let project = ui.project.get_untracked();
    let name = state.selected.get_untracked();
    if name.is_empty() {
        return;
    }
    let mut doc = if let Some(existing) = state.studio_doc.get_untracked() {
        existing
    } else {
        let Some(raw) = state
            .drafts
            .with_untracked(|d| d.get(&project).and_then(|d| d.sources.get(&name)).cloned())
        else {
            return;
        };
        let Ok(parsed) = canban_workflow_engine::source::parse(&raw) else {
            return;
        };
        parsed
    };
    mutate(&mut doc);
    if let Ok(encoded) = canban_workflow_engine::source::encode(&doc) {
        state
            .studio_synced
            .replace((project.clone(), name.clone(), encoded.clone()));
        state.studio_error.set(String::new());
        state.studio_doc.set(Some(doc));
        state.drafts.update(|d| {
            if let Some(d) = d.get_mut(&project) {
                d.sources.insert(name, encoded);
            }
        });
    }
}

fn visual_studio(ui: Ui) -> AnyElement {
    studio::render(ui)
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
                .highlighting(true)
                .line_numbers(true)
                .indent_guides(true)
                .auto_indent(true)
                .auto_close(true)
                .tab_size(Some(2)),
        )
        .into_element()
}
/// Real native modal surface: editor layers behind it cannot paint through it.
pub(crate) fn workflow_dialog(
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
    let default_prof = computed(move || {
        let _ = (ui.default_agent.get(), ui.profiles.get());
        crate::sessions::default_profile(ui)
    });
    spawn(async move {
        next_frame().await;
        if state.agent.try_get() == Some(true)
            && let Some(api) = prompt_editor.api::<nsh_base::EditorApi>()
        {
            api.focus();
        }
    });
    workflow_dialog(view().flex_1().min_h(px(0.)).gap_3()
        .child(view().flex_row().items_center().gap_3().child(icon(Icon::Agent)).child(heading("Edit with agent")).child(badge(move || format!("@{}", default_prof.get()))).child(view().flex_1()).child(chrome_button(Icon::Close,"Close agent editor",move|_|state.agent.set(false))))
        .child(muted("Review this Project-wide Prompt, then copy it or start a configuration conversation in Terminal with your default Agent."))
        .child(view().flex_1().min_h(px(0.)).bg(INSET).p_2().child(nsh_base::editor().value(move||Some(state.prompt.get().into())).read_only(true).soft_wrap(true).line_numbers(false).component_ref(&prompt_editor).accessibility_label("Project Workflow configuration Prompt").size_full()))
        .child(when(move || !state.error.get().is_empty(), move || notice(move || state.error.get(), Tone::Attention)))
        .child(muted(move||state.copy_status.get()))
        .child(view().flex_row().flex_wrap().gap_2().child(action("Select all",Variant::Secondary,move|_|{if let Some(api)=prompt_editor.api::<nsh_base::EditorApi>(){api.focus();api.run(nsh_base::EditorCommand::SelectAll);}})).child(action("Copy prompt",Variant::Secondary,move|_|copy_prompt(ui))).child(view().flex_1()).child(action(move||{
            let prof=default_prof.get();
            let key=format!("configuration:{}:workflows:{prof}",ui.project.get());ui.snapshot.with(|s|{if let Some(c)=s.configuration_sessions.iter().find(|c|c.id==key){if c.state=="finished"{"Resume and open Terminal"}else{"Continue in Terminal"}}else{"Start and open Terminal"}})
        },Variant::Primary,move|_|launch(ui,false)).disabled(move||ui.pending.get()||default_prof.get().is_empty())).child(action("New conversation",Variant::Quiet,move|_|launch(ui,true)).disabled(move||ui.pending.get()||default_prof.get().is_empty()))), move || state.agent.set(false))
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
    if !crate::backend::is_connected() {
        return;
    }
    ui.spawn(async move {
        let result = blocking(move || {
            crate::routed_call(
                "workflow.list",
                json!({"project_id":project,"workflow_id":workflow}),
                None,
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

/// Read-only Board column header pill displaying the column's active automation
/// intent (`⚡ Auto develop`, `⚡ Auto merge`, etc.) with a hover tooltip summary.
/// Quiet/manual columns (`None`) render nothing.
pub fn column_automation_pill(ui: Ui, column: String) -> impl IntoElement {
    let info = computed(move || {
        let project = ui.project.get();
        ui.snapshot.with(|s| {
            let files: Vec<_> = s
                .workflow_sources
                .iter()
                .filter(|f| f.project_id == project)
                .cloned()
                .collect();
            canban_workflow_engine::source::column_automation(
                &column,
                &files,
                &s.workflow_revisions,
                &s.event_subscriptions,
            )
        })
    });
    when(
        move || info.get().is_some(),
        move || {
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .min_w(px(0.))
                .max_w(px(148.))
                .px_1p5()
                .py_0p5()
                .rounded(px(10.))
                .bg(INSET)
                .label(move || {
                    info.get()
                        .map(|a| format!("Column automation · {}", a.label))
                        .unwrap_or_default()
                })
                .tooltip(
                    tooltip_box()
                        .placement(Placement::Bottom)
                        .max_w(px(320.))
                        .child(
                            text(move || info.get().map(|a| a.tooltip).unwrap_or_default())
                                .text_size(px(CAPTION_SIZE))
                                .text_color(TEXT),
                        ),
                )
                .child(icon_tinted(Icon::Zap, ACCENT).w(px(11.)).h(px(11.)))
                .child(
                    text(move || info.get().map(|a| a.label).unwrap_or_default())
                        .text_size(px(CAPTION_SIZE))
                        .text_color(MUTED)
                        .truncate()
                        .min_w(px(0.)),
                )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::{ApproxTextEngine, Core, MountItem, geometry::size};
    use std::collections::HashMap;

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

    #[test]
    fn yaml_source_editor_enables_syntax_highlighting_despite_markdown_input_default() {
        #[component]
        fn ProbeRoot() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            canban_ui_kit::markdown::install_editor_language();
            let ui = Ui::new();
            let project_id = "proj-yaml".to_owned();
            let yaml = "# Workflow comment\nschema: 1\nid: delivery\nname: \"Delivery\"\non:\n  develop:\n    event: card.column_entered\n    start_at: build\nsteps:\n  - id: build\n    uses: agent\n    with:\n      profile: developer\n";
            let mut draft = Draft {
                base: json!({"revision": "r1", "sources": [{"filename": "development.yaml", "source": yaml}]}),
                ..Default::default()
            };
            draft
                .sources
                .insert("development.yaml".into(), yaml.to_owned());
            draft.selected = "development.yaml".into();
            ui.project.set(project_id.clone());
            ui.page.set("workflows".into());
            ui.workflows.mode.set("yaml".into());
            ui.workflows.selected.set("development.yaml".into());
            ui.workflows.drafts.update(|d| {
                d.insert(project_id, draft);
            });
            view().w(px(960.)).h(px(640.)).child(source_editor(ui))
        }

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .build()
            .unwrap();
        let mut core = Core::new(ApproxTextEngine, size(px(960.), px(640.)));
        core.set_components(nsh_base::registry());
        core.set_task_runtime(runtime.handle().clone());
        core.mount_root(&nsh_core::element::Root::of::<ProbeRoot>());
        let mut texts = HashMap::new();
        if let Some(batch) = core.commit() {
            for item in batch.items {
                if let MountItem::Text { id, text, .. } = item {
                    texts.insert(id, text);
                }
            }
        }
        // Highlighting runs off-thread in NSH; drive component tasks until the
        // background job finishes and emits multi-color AttributedText spans.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            core.run_component_tasks();
            if let Some(batch) = core.commit() {
                for item in batch.items {
                    if let MountItem::Text { id, text, .. } = item {
                        texts.insert(id, text);
                    }
                }
            }
            let distinct_syntax_colors: std::collections::HashSet<_> = texts
                .values()
                .filter(|t| {
                    t.text.contains("schema:")
                        || t.text.contains("# Workflow comment")
                        || t.text.contains("name: \"Delivery\"")
                })
                .flat_map(|t| t.spans.iter().map(|s| format!("{:?}", s.style.color)))
                .collect();
            if distinct_syntax_colors.len() >= 3 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        panic!(
            "expected YAML syntax highlighting rows with >= 3 distinct span colors, got: {:?}",
            texts
                .values()
                .map(|t| (
                    &t.text,
                    t.spans.iter().map(|s| s.style.color).collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
        );
    }
}
