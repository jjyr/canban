use crate::presenter::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::json;

fn layout(
    title: impl IntoReactive<SharedString>,
    subtitle: impl IntoReactive<SharedString>,
) -> View {
    view()
        .flex_1()
        .p_5()
        .gap_4()
        .min_h(px(0.0))
        .child(view().gap_1().child(heading(title)).child(muted(subtitle)))
}
pub fn screen(ui: Ui, page: &str) -> AnyElement {
    match page {
        "new-project" => new_project(ui).into_element(),
        "new-card" => new_card(ui).into_element(),
        "notes" => crate::notes::screen(ui).into_element(),
        "overview" | "agents" => crate::overview::screen(ui).into_element(),
        "board" | "card" | "terminal" | "review" => crate::workspace::screen(ui).into_element(),
        "archive" => archived(ui).into_element(),
        "settings" => settings(ui).into_element(),
        _ => crate::workspace::screen(ui).into_element(),
    }
}
fn new_project(ui: Ui) -> impl IntoElement {
    let name = signal(String::new());
    let directory = signal(String::new());
    layout("New Project", "A source directory holds the work; Canban keeps its Board, Notes and history.")
        .child(scroll().flex_1().min_h(px(0.)).child(
            settings_section("Project details", "Choose an existing folder or enter a new directory path. Git is optional.")
                .max_w(px(720.))
                .child(settings_field("Project name", "You can change the suggested name before creating the Project.",
                    field(name, "Name").label("Project name")))
                .child(settings_field("Source directory", "A directory that does not exist will be created when you submit.",
                    directory_field(directory, move |path| {
                        if name.try_get().is_some_and(|s| s.trim().is_empty())
                            && let Some(basename) = path.file_name() {
                            name.try_set(basename.to_string_lossy().into_owned());
                        }
                    })))
                .child(view().flex_row().flex_wrap().gap_2()
                    .child(action("Create Project", Variant::Primary, move |_| {
                        dispatch(ui, "project.create", json!({"name": name.get_untracked().trim(), "directory": directory.get_untracked().trim()}));
                    }).disabled(move || ui.pending.get() || name.get().trim().is_empty() || directory.get().trim().is_empty()))
                    .child(action("Cancel", Variant::Quiet, move |_| ui.navigate("board"))))
        ))
}

fn new_card(ui: Ui) -> impl IntoElement {
    let project = ui.project.get_untracked();
    let source = ui.source_note.get_untracked();
    let note = ui
        .snapshot
        .with_untracked(|s| s.notes.iter().find(|n| n.id == source).cloned());
    let key = format!("new-card:{project}:{source}");
    let initial = note
        .as_ref()
        .map(|n| crate::drafts::Draft {
            title: n.title.clone(),
            body: n.body.clone(),
            ..Default::default()
        })
        .unwrap_or_default();
    let draft = crate::drafts::document(ui, key.clone(), initial);
    let mut form = layout("New Card", "Define the scope, then create it in Backlog.").child(
        action("← Back", Variant::Quiet, move |_| {
            if source.is_empty() {
                ui.page.set("board".into());
            } else {
                ui.open_note(source.clone());
            }
        }),
    );
    if let Some(n) = note {
        form = form.child(notice(
            format!("Source Note: {} · included in this Card's context", n.title),
            Tone::Info,
        ));
    }
    form.child(panel().max_w(px(800.0)).child(bound_field(move||draft.get().title,move|s|draft.update(|d|d.title=s.into()),"Card title"))
        .child(bound_field(move||draft.get().body,move|s|draft.update(|d|d.body=s.into()),"Task description").multiline(true).h(px(220.0)))
        .child(bound_field(move||draft.get().tags,move|s|draft.update(|d|d.tags=s.into()),"Routing tags"))
        .child(muted(move||draft.get().error))
        .child(action("Create in Backlog",Variant::Primary,move|_|{
            let value=draft.get_untracked();let mut p=json!({"project_id":project,"title":value.title,"body":value.body,"tags":value.tags.split(',').map(str::trim).filter(|s|!s.is_empty()).collect::<Vec<_>>()});
            let source=ui.source_note.get_untracked();if !source.is_empty(){p["source_note_id"]=json!(source);}
            let key=key.clone();
            dispatch_complete(ui,"card.create",p,move|result|match result{Ok(_)=>{ui.drafts.with(|m|{m.remove(&key);});},Err(e)=>{if let Some(mut value)=draft.try_get(){value.error=e;draft.try_set(value);}}});
        }).disabled(move||ui.pending.get() || draft.with(|d|d.title.trim().is_empty()))))
}
fn archived(ui: Ui) -> impl IntoElement {
    layout("Archived Cards", move || {
        let _ = ui.project.get();
        ui.current_project()
            .map(|p| p.name)
            .unwrap_or_else(|| "No Project selected".into())
    })
    .child(scroll().flex_1().child(dynamic(move || {
        let project = ui.project.get();
        view().gap_3().children(ui.snapshot.with(|s| {
            s.cards
                .iter()
                .filter(|c| c.project_id == project && c.archived_at.is_some())
                .map(|c| {
                    let id = c.id.clone();
                    panel()
                        .child(label(format!("#{} · {}", c.number, c.title)))
                        .child(muted(c.archive.clone().unwrap_or_else(|| {
                            c.archive_error
                                .clone()
                                .unwrap_or_else(|| "Compression pending".into())
                        })))
                        .child(action(
                            "Open archived Card",
                            Variant::Secondary,
                            move |_| ui.open_card(id.clone()),
                        ))
                        .into_element()
                })
                .collect::<Vec<_>>()
        }))
    })))
}
fn settings(ui: Ui) -> impl IntoElement {
    let home = crate::APP
        .get()
        .map(|app| app.home.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/preview/.canban".into());
    let runtime = crate::APP.get().map(|app| app.config()).unwrap_or_default();
    layout("Settings", "Application preferences and JSON configuration.")
        .child(scroll().flex_1().min_h(px(0.)).child(view().w_full().max_w(px(880.)).min_w(px(0.)).gap_4()
            .child(crate::components::scheduling::settings(ui))
            .child(settings_section("Agent profiles", "Define Agents once; workflow stages and @mentions reference these profiles.")
                .child(settings_field("Configuration", "Command and profile changes apply after reloading, to newly queued work.",
                    muted(format!("{home}/config/agents.json"))))
                .child(crate::profiles::registry_items(ui.profiles))
                .child(view().items_start().child(action("Reload configuration", Variant::Secondary, move |_| dispatch(ui, "config.reload", json!({}))))))
            .child(settings_section("Workflow", "New Projects use this default template. Existing Projects retain their workflow.")
                .child(settings_field("Default workflow configuration", "Edit this JSON file to customize stages, profiles and workspace behavior.",
                    muted(format!("{home}/config/workflows/default.json")))))
            .child(settings_section("Herdr", "Terminal runtime connection; configure it in agents.json.")
                .child(settings_field("Executable", "Used to connect to the configured runtime.", muted(runtime.hc)))
                .child(settings_field("Session", "The configured Herdr runtime session.", muted(runtime.hc_session)))
                .child(view().items_start().child(action("Check connection", Variant::Secondary, move |_| dispatch(ui, "herdr.status", json!({}))))))
            .child(settings_section("Storage", "Canban keeps structured data, configuration and managed artifacts in its application home.")
                .child(settings_field("Application home", "Configuration, documents and execution history are stored here.", muted(home))))
            .child(muted("Closing Canban stops scheduling. Herdr-owned programs can continue; reopen Canban to reconcile their state."))
        ))
}
