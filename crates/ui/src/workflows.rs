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
                    state.latest.set(v);
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
                    ui.status.set(String::new());
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
    dispatch_then(
        ui,
        "configuration.open",
        json!({"project_id":ui.project.get_untracked(),"profile_id":state.profile.get_untracked(),"prompt":state.prompt.get_untracked(),"fresh":fresh}),
        move |v| {
            if ui.location() == origin
                && let Some(id) = v["terminal_id"].as_str()
            {
                state.agent.set(false);
                state.history_open.set(false);
                ui.open_terminal(id.into());
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
    view().flex_1().min_h(px(0.)).min_w(px(0.)).p_5().gap_3()
        .child(view().flex_row().items_center().gap_3().child(action("← Board",Variant::Quiet,move|_|ui.navigate("board"))).child(heading(move||format!("{} / Workflows",ui.current_project().map(|p|p.name).unwrap_or_default()))).child(view().flex_1()).child(action("+ New workflow",Variant::Secondary,move|_|{
            let project=ui.project.get_untracked();let mut name=String::new();state.drafts.update(|d|{let draft=d.entry(project).or_default();let mut n=1;while draft.sources.contains_key(&format!("workflow-{n}.yaml")){n+=1;}name=format!("workflow-{n}.yaml");draft.sources.insert(name.clone(),format!("schema: 1\nid: workflow-{}\nname: New workflow\non:\n  manual:\n    event: manual\n    start_at: explain\nsteps:\n  - id: explain\n    uses: agent\n    with:\n      profile: {}\n      instruction: |\n        Read the Card and propose a concrete plan.\n",canban_types::id(),ui.profiles.with_untracked(|p|p.first().map(|p|p.id.clone()).unwrap_or_default())));draft.selected=name.clone();});state.selected.set(name);
        })).child(action("Edit with agent",Variant::Primary,move|_|edit_agent(ui))))
        .child(when(move||!state.error.get().is_empty(),move||notice(move||state.error.get(),Tone::Attention)))
        .child(when(move||!state.latest.get().is_null(),move||view().gap_2().child(muted("Latest sources are available. Your edits are retained; compare before reloading.")).child(scroll().max_h(px(160.)).child(read_dynamic(move||state.latest.with(|v|v["sources"].as_array().into_iter().flatten().map(|f|format!("{}\n{}",f["filename"].as_str().unwrap_or_default(),f["source"].as_str().unwrap_or_default())).collect::<Vec<_>>().join("\n\n")))))))
        .child(view().flex_1().min_h(px(0.)).flex_row().gap_4()
            .child(scroll().w(px(210.)).min_h(px(0.)).child(view().gap_2().child(each(move||names.get(),|name|name.clone(),move|name,_|{let name=name.get_untracked();let select=name.clone();let selected=name.clone();action(name,Variant::Quiet,move|_|{let project=ui.project.get_untracked();state.drafts.update(|d|{if let Some(d)=d.get_mut(&project){d.selected=select.clone();}});state.selected.set(select.clone());}).bg(move||if state.selected.get()==selected{SELECTED}else{CLEAR})}))))
            .child(view().flex_1().min_w(px(0.)).min_h(px(0.)).gap_2().child(view().flex_row().items_center().gap_2().child(label(move||state.selected.get())).child(muted(move||{
                let project=ui.project.get();let name=state.selected.get();state.drafts.with(|d|d.get(&project).and_then(|d|d.base["sources"].as_array()).and_then(|rows|rows.iter().find(|r|r["filename"]==name)).map(|r|format!("Active v{}",r["active"]["revision"])).unwrap_or_else(||"New draft".into()))
            })).child(action("History",Variant::Quiet,move|_|show_history(ui)))).child(dynamic(move||{
                let name=state.selected.get();let project=ui.project.get_untracked();
                let initial=state.drafts.with_untracked(|d|d.get(&project).and_then(|d|d.sources.get(&name)).cloned()).unwrap_or_default();
                let text=signal(initial);let editor=canban_ui_kit::markdown::MarkdownInput::new();
                editor.restore_after(computed(move||state.agent.get()||state.history_open.get()||ui.quick_open.get()),computed(move||ui.page.get()=="workflows"));
                let sync_name=name.clone();let sync_project=project.clone();
                effect(move||{let saved=state.drafts.with(|d|d.get(&sync_project).and_then(|d|d.sources.get(&sync_name)).cloned());if let Some(saved)=saved && saved!=text.get_untracked(){text.set(saved);}});
                effect(move||{let value=text.get();state.drafts.update(|d|{if let Some(d)=d.get_mut(&project){d.sources.insert(name.clone(),value);}});});
                view().flex_1().min_h(px(0.)).bg(SURFACE).rounded(px(RADIUS)).p_2().child(editor.source(text,"Workflow YAML","Select or create a Workflow…")).into_element()
            }))))
        .child(view().flex_row().items_center().gap_2().child(action("Refresh / compare latest",Variant::Quiet,move|_|load(ui,false))).child(action("Discard changes / reload",Variant::Secondary,move|_|load(ui,true)).disabled(move||!dirty.get() && state.latest.get().is_null())).child(view().flex_1()).child(action("Save and apply",Variant::Primary,move|_|save(ui)).disabled(move||ui.pending.get()||state.loading.get()||!apply.get())))
        .child(when(move||state.agent.get(),move||agent_panel(ui)))
        .child(when(move||state.history_open.get(),move||history_panel(ui)))
}
/// Shared production panel, also exercised in the native Gallery.
pub fn agent_panel(ui: Ui) -> impl IntoElement {
    on_key_with(
        "escape",
        KeyOptions {
            when: nsh_core::keymap::When::Always,
            ..Default::default()
        },
        move |_| ui.workflows.agent.set(false),
    );
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
    view().absolute().inset(px(16.)).p_5().gap_3().bg(SURFACE).border_1().border_color(BORDER).rounded(px(RADIUS))
        .child(view().flex_row().items_center().gap_3().child(heading("Edit with agent")).child(view().flex_1()).child(action("Close",Variant::Quiet,move|_|state.agent.set(false))))
        .child(muted("Review this Project-wide Prompt, then copy it or start a configuration conversation in Terminal."))
        .child(view().flex_row().flex_wrap().gap_2().child(each(move||ui.profiles.get(),|p|p.id.clone(),move|p,_|{let p=p.get_untracked();let id=p.id.clone();let selected=p.id.clone();action(format!("@{}",p.id),Variant::Secondary,move|_|state.profile.set(id.clone())).bg(move||if state.profile.get()==selected{SELECTED}else{INSET})})))
        .child(view().flex_1().min_h(px(0.)).bg(INSET).p_2().child(nsh_base::editor().value(move||Some(state.prompt.get().into())).read_only(true).soft_wrap(true).line_numbers(false).component_ref(&prompt_editor).accessibility_label("Project Workflow configuration Prompt").size_full()))
        .child(muted(move||state.copy_status.get()))
        .child(view().flex_row().flex_wrap().gap_2().child(action("Select all",Variant::Secondary,move|_|{if let Some(api)=prompt_editor.api::<nsh_base::EditorApi>(){api.focus();api.run(nsh_base::EditorCommand::SelectAll);}})).child(action("Copy prompt",Variant::Secondary,move|_|copy_prompt(ui))).child(view().flex_1()).child(action(move||{
            let key=format!("configuration:{}:workflows:{}",ui.project.get(),state.profile.get());ui.snapshot.with(|s|{if let Some(c)=s.configuration_sessions.iter().find(|c|c.id==key){if c.state=="finished"{"Resume and open Terminal"}else{"Continue in Terminal"}}else{"Start and open Terminal"}})
        },Variant::Primary,move|_|launch(ui,false)).disabled(move||ui.pending.get()||state.profile.get().is_empty())).child(action("New conversation",Variant::Quiet,move|_|launch(ui,true)).disabled(move||ui.pending.get()||state.profile.get().is_empty())))
}
/// Pure active-definition projection; no source text or prose RPC is needed.
pub fn column_trigger(ui: Ui, column: String) -> impl IntoElement {
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
                .relative()
                .child(
                    view()
                        .tooltip(
                            nsh_base::tooltip().child(label(move || details.get().join("\n\n"))),
                        )
                        .child(chrome_button(Icon::Trigger, "Column triggers", move |_| {
                            open.update(|v| *v = !*v)
                        })),
                )
                .child(when(
                    move || open.get(),
                    move || {
                        view()
                            .absolute()
                            .top(px(32.))
                            .right(px(0.))
                            .w(px(340.))
                            .p_3()
                            .gap_2()
                            .bg(SURFACE)
                            .border_1()
                            .border_color(BORDER)
                            .rounded(px(RADIUS))
                            .child(label("Column triggers"))
                            .child(read_dynamic(move || details.get().join("\n\n")))
                            .child(action(
                                "View Workflows source",
                                Variant::Secondary,
                                move |_| ui.navigate("workflows"),
                            ))
                            .child(action("Close", Variant::Quiet, move |_| open.set(false)))
                    },
                ))
        },
    )
}

fn read_dynamic(body: impl Fn() -> String + 'static) -> impl IntoElement {
    dynamic(move || read_text(body()).into_element())
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
    on_key_with(
        "escape",
        KeyOptions {
            when: nsh_core::keymap::When::Always,
            ..Default::default()
        },
        move |_| state.history_open.set(false),
    );
    view().absolute().inset(px(16.)).p_5().gap_3().bg(SURFACE).border_1().border_color(BORDER).rounded(px(RADIUS))
        .child(view().flex_row().items_center().gap_2().child(heading(move||format!("{} / History",state.history_name.get()))).child(view().flex_1()).child(action("Close",Variant::Quiet,move|_|state.history_open.set(false))))
        .child(muted("Published SQL revisions are immutable. Loading an earlier version creates an editor draft; Save and apply publishes a new version."))
        .child(view().flex_1().min_h(px(0.)).flex_row().gap_3().child(scroll().w(px(190.)).child(each(move||state.history.get(),|w|w.id.clone(),move|w,_|{let revision=w.get_untracked().revision;action(format!("Version {revision}"),Variant::Secondary,move|_|state.history_revision.set(revision)).bg(move||if state.history_revision.get()==revision{SELECTED}else{INSET})})))
            .child(view().flex_1().min_h(px(0.)).child(nsh_base::editor().value(move||Some(state.history.with(|h|h.iter().find(|w|w.revision==state.history_revision.get()).and_then(|w|w.source_yaml.clone()).unwrap_or_default()).into())).read_only(true).soft_wrap(true).line_numbers(false).folding(false).component_ref(&editor).accessibility_label("Historical Workflow YAML").size_full())))
        .child(view().flex_row().justify_end().child(action("Load as draft",Variant::Primary,move|_|{
            if let Some(raw)=state.history.with_untracked(|h|h.iter().find(|w|w.revision==state.history_revision.get_untracked()).and_then(|w|w.source_yaml.clone())){
                let name=state.history_name.get_untracked();let project=ui.project.get_untracked();state.drafts.update(|d|{if let Some(d)=d.get_mut(&project){d.sources.insert(name,raw);}});state.history_open.set(false);
            }
        })))
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
