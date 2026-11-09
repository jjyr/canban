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

fn new_card(ui: Ui) -> impl IntoElement {
    crate::github::new_card(ui)
}
fn archived_row(ui: Ui, card: canban_types::Card) -> View {
    let id = card.id.clone();
    let mut row = panel().child(label(format!("#{} · {}", card.number, card.title)));
    if let Some(error) = &card.archive_error {
        row = row.child(notice(error.clone(), Tone::Attention));
    }
    row.child(
        view()
            .flex_row()
            .gap_2()
            .child(action("Open Card", Variant::Secondary, move |_| {
                ui.open_card(id.clone())
            }))
            .child(
                action("Restore", Variant::Quiet, move |_| {
                    crate::card_actions::apply(
                        ui,
                        card.clone(),
                        crate::card_actions::Action::Restore,
                    )
                })
                .disabled(move || ui.pending.get()),
            ),
    )
}
fn archived(ui: Ui) -> impl IntoElement {
    let removed = signal(Vec::<canban_types::Card>::new());
    let error = signal(String::new());
    let epoch = signal(0_u64);
    let active = computed(move || {
        ui.snapshot.with(|s| {
            s.cards
                .iter()
                .map(|c| (c.id.clone(), c.revision))
                .collect::<Vec<_>>()
        })
    });
    effect(move || {
        let project = ui.project.get();
        let _ = active.get();
        let Some(app) = crate::APP.get().cloned() else {
            removed.set(ui.snapshot.with_untracked(|s| {
                s.cards
                    .iter()
                    .filter(|c| c.project_id == project && c.removed_at.is_some())
                    .cloned()
                    .collect()
            }));
            return;
        };
        let ticket = epoch.get_untracked() + 1;
        epoch.set(ticket);
        removed.set(Vec::new());
        error.set(String::new());
        ui.spawn(async move {
            let result = blocking(move || {
                app.call(
                    "card.list",
                    json!({"project_id":project,"include_removed":true}),
                )
                .and_then(|v| Ok(serde_json::from_value::<Vec<canban_types::Card>>(v)?))
            })
            .await;
            if epoch.try_get() != Some(ticket) {
                return;
            }
            match result {
                Ok(Ok(cards)) => {
                    removed.set(
                        cards
                            .into_iter()
                            .filter(|c| c.removed_at.is_some())
                            .collect(),
                    );
                }
                Ok(Err(e)) => {
                    error.set(format!("{e:#}"));
                }
                Err(e) => {
                    error.set(e.to_string());
                }
            }
        });
    });
    layout("Archived Cards", move || {
        let _ = ui.project.get();
        ui.current_project().map(|p| p.name).unwrap_or_else(|| "No Project selected".into())
    }).child(scroll().flex_1().child(view().w_full().max_w(px(880.)).gap_3()
        .child(dynamic(move || {
            let project=ui.project.get();
            view().gap_3().children(ui.snapshot.with(|s| s.cards.iter().filter(|c|c.project_id==project && c.archived_at.is_some() && c.removed_at.is_none()).map(|c|{
                archived_row(ui,c.clone()).into_element()
            }).collect::<Vec<_>>()))
        }))
        .child(when(move || !removed.get().is_empty(),move || {
            view().gap_3().child(label("Deleted Cards")).child(dynamic(move || {
                view().gap_3().children(removed.get().into_iter().map(|c|{
                    panel().child(label(format!("#{} · {}",c.number,c.title)))
                        .child(view().flex_row().child(action("Restore",Variant::Quiet,move |_|crate::card_actions::apply(ui,c.clone(),crate::card_actions::Action::Restore)).disabled(move || ui.pending.get()))).into_element()
                }).collect::<Vec<_>>())
            }))
        }))
        .child(when(move || !error.get().is_empty(),move || notice(error.get(),Tone::Attention)))
    ))
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
