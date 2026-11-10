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
        "new-project" => crate::projects::new_project(ui).into_element(),
        "terminals" | "terminal" => crate::terminals::screen(ui).into_element(),
        "overview" | "agents" => crate::overview::screen(ui).into_element(),
        "board" | "card" | "review" | "new-card" => crate::workspace::screen(ui).into_element(),
        "drafts" => crate::drafts::screen(ui).into_element(),
        "archive" => crate::archive::screen(ui).into_element(),
        "settings" => settings(ui).into_element(),
        "github" => crate::github::screen(ui).into_element(),
        "github-account" => crate::github::account(ui).into_element(),
        "github-repository" => crate::github::repository_screen(ui).into_element(),
        _ => crate::workspace::screen(ui).into_element(),
    }
}

fn settings(ui: Ui) -> impl IntoElement {
    let home = crate::APP
        .get()
        .map(|app| app.home.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/preview/.canban".into());
    let runtime = crate::APP.get().map(|app| app.config()).unwrap_or_default();
    layout("Settings", "Application preferences and JSON configuration.")
        .child(scroll().flex_1().min_h(px(0.)).child(view().w_full().max_w(px(880.)).min_w(px(0.)).gap_4()
            .child(crate::utilities::settings(ui))
            .child(crate::updates::settings(ui))
            .child(crate::components::scheduling::settings(ui))
            .child(crate::machines::settings(ui))
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
