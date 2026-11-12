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
        "workflows" => crate::workflows::screen(ui).into_element(),
        "machines" => view()
            .flex_1()
            .min_h(px(0.))
            .p_5()
            .gap_4()
            .child(subpage_header(
                "Back to Settings",
                "Settings / Machines",
                move |_| ui.navigate("settings"),
            ))
            .child(
                scroll().flex_1().min_h(px(0.)).child(
                    view()
                        .w_full()
                        .max_w(px(880.))
                        .child(crate::machines::settings(ui)),
                ),
            )
            .into_element(),
        "project-location" => crate::machines::project_location(ui).into_element(),
        "new-project" => crate::projects::new_project(ui).into_element(),
        "terminals" | "terminal" => crate::terminals::screen(ui).into_element(),
        "overview" | "agents" => crate::overview::screen(ui).into_element(),
        "review" => crate::review::screen(ui).into_element(),
        "board" | "card" | "new-card" => crate::workspace::screen(ui).into_element(),
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
    let home_for_copy = home.clone();
    let config_path = format!("{home}/config/agents.json");
    let config_for_copy = config_path.clone();
    let runtime = crate::APP.get().map(|app| app.config()).unwrap_or_default();
    layout("Settings", "Application preferences and Agent configuration.")
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                view()
                    .w_full()
                    .max_w(px(880.))
                    .min_w(px(0.))
                    .gap_7()
                    .pb_6()
                    .child(crate::utilities::settings(ui))
                    .child(crate::utilities::terminal_settings(ui))
                    .child(crate::updates::settings(ui))
                    .child(crate::components::scheduling::settings(ui))
                    .child(crate::machines::settings(ui))
                    .child(
                        settings_section(
                            "Agent profiles",
                            "Define Agents once; workflow stages and @mentions reference these profiles.",
                        )
                        .trailing(compact_action(
                            "Reload configuration",
                            Variant::Secondary,
                            move |_| dispatch(ui, "config.reload", json!({})),
                        ))
                        .child(crate::profiles::registry_items(ui.profiles))
                        .child(settings_field(
                            "Configuration",
                            "Command and profile changes apply after reloading, to newly queued work.",
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(
                                    label(config_path)
                                        .font_family("Menlo")
                                        .text_size(px(12.)),
                                )
                                .child(compact_action(
                                    "Copy path",
                                    Variant::Quiet,
                                    move |_| copy_text(ui, config_for_copy.clone()),
                                )),
                        )),
                    )
                    .child(
                        settings_section(
                            "Herdr",
                            "Terminal runtime connection; configure it in agents.json.",
                        )
                        .trailing(compact_action(
                            "Check connection",
                            Variant::Secondary,
                            move |_| dispatch(ui, "herdr.status", json!({})),
                        ))
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .gap_6()
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(220.))
                                        .child(settings_field(
                                            "Executable",
                                            "Used to connect to the configured runtime.",
                                            label(runtime.hc)
                                                .font_family("Menlo")
                                                .text_size(px(12.)),
                                        )),
                                )
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(220.))
                                        .child(settings_field(
                                            "Session",
                                            "The configured Herdr runtime session.",
                                            label(runtime.hc_session)
                                                .font_family("Menlo")
                                                .text_size(px(12.)),
                                        )),
                                ),
                        ),
                    )
                    .child(
                        settings_section(
                            "Storage",
                            "Canban keeps structured data, configuration and managed artifacts in its application home.",
                        )
                        .trailing(compact_action(
                            "Copy path",
                            Variant::Quiet,
                            move |_| copy_text(ui, home_for_copy.clone()),
                        ))
                        .child(settings_field(
                            "Application home",
                            "Configuration, documents and execution history are stored here.",
                            label(home).font_family("Menlo").text_size(px(12.)),
                        )),
                    )
                    .child(muted(
                        "Closing this window keeps AppServer and scheduling running. Herdr owns the terminal programs.",
                    )),
            ),
        )
}
