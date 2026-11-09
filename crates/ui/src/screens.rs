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
        "machines" => view()
            .flex_1()
            .p_5()
            .child(scroll().flex_1().child(crate::machines::settings(ui)))
            .into_element(),
        "project-location" => crate::machines::project_location(ui).into_element(),
        "new-project" => new_project(ui).into_element(),
        "new-card" => new_card(ui).into_element(),
        "notes" => crate::notes::screen(ui).into_element(),
        "terminals" | "terminal" => crate::terminals::screen(ui).into_element(),
        "overview" | "agents" => crate::overview::screen(ui).into_element(),
        "board" | "card" | "review" => crate::workspace::screen(ui).into_element(),
        "archive" => archived(ui).into_element(),
        "settings" => settings(ui).into_element(),
        _ => crate::workspace::screen(ui).into_element(),
    }
}
fn new_project(ui: Ui) -> impl IntoElement {
    let name = signal(String::new());
    let directory = signal(String::new());
    let machine = signal("local".to_owned());
    layout("New Project", "A source directory holds the work; Canban keeps its Board, Notes and history.")
        .child(scroll().flex_1().min_h(px(0.)).child(
            settings_section("Project details", "Choose an existing folder or enter a new directory path. Git is optional.")
                .max_w(px(720.))
                .child(settings_field("Project name", "You can change the suggested name before creating the Project.",
                    field(name, "Name").label("Project name")))
                .child(crate::machines::location_fields(ui,machine,directory))
                .child(view().flex_row().flex_wrap().gap_2()
                    .child(action("Create Project", Variant::Primary, move |_| {
                        dispatch(ui, "project.create", json!({"name": name.get_untracked().trim(), "location":{"machine":machine.get_untracked(),"directory": directory.get_untracked().trim()}}));
                    }).disabled(move || ui.pending.get() || name.get().trim().is_empty() || directory.get().trim().is_empty()))
                    .child(action("Cancel", Variant::Quiet, move |_| ui.navigate("board"))))
        ))
}

fn new_card(ui: Ui) -> impl IntoElement {
    crate::github::new_card(ui)
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
            .child(crate::machines::settings(ui))
            .child(crate::github::settings(ui))
            .child(settings_section("Project work location","Change where future work runs for the selected Project.").child(action("Edit work location…",Variant::Secondary,move |_|ui.navigate("project-location"))))
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
            .child(muted("Closing this window keeps AppServer and scheduling running. Herdr owns the terminal programs."))
        ))
}
