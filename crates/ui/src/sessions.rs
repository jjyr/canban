//! One reusable Card/profile entry and native terminal screen.
use crate::{components::run_panel, presenter::*};
use canban_types::Request;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::json;

pub fn card_agents(ui: Ui) -> impl IntoElement {
    let more = signal(false);
    view()
        .gap_2()
        .child(
            view()
                .flex_row()
                .justify_between()
                .items_center()
                .child(label("Agents"))
                .child(action("Other Agents", Variant::Quiet, move |_| {
                    more.set(!more.get_untracked())
                })),
        )
        .child(dynamic(move || {
            let card = ui.card.get();
            let snapshot = ui.snapshot.get();
            let show_all = more.get();
            let stage_profile = snapshot
                .cards
                .iter()
                .find(|c| c.id == card)
                .and_then(|c| {
                    snapshot
                        .projects
                        .iter()
                        .find(|p| p.id == c.project_id)
                        .and_then(|p| p.workflow.stages.iter().find(|s| s.id == c.stage))
                })
                .and_then(|s| s.profile.clone());
            let profiles = ui
                .profiles
                .get()
                .into_iter()
                .filter(|p| {
                    show_all
                        || stage_profile.as_deref() == Some(&p.id)
                        || snapshot
                            .requests
                            .iter()
                            .any(|r| r.card_id == card && r.profile.id == p.id)
                })
                .collect::<Vec<_>>();
            let mut rows = view().gap_2();
            if profiles.is_empty() {
                rows = rows.child(muted("Choose an Agent to discuss or work on this Card."));
            }
            for profile in profiles {
                let latest = snapshot
                    .requests
                    .iter()
                    .rev()
                    .find(|r| r.card_id == card && r.profile.id == profile.id);
                let run = latest.and_then(|r| {
                    snapshot
                        .attempts
                        .iter()
                        .rev()
                        .find(|a| a.request_id == r.id)
                });
                let exited = run.is_some_and(|a| {
                    a.observed
                        .as_ref()
                        .is_some_and(|s| s["state"] == "finished")
                });
                let state = if exited {
                    "Resume"
                } else if run.is_some() {
                    "Open terminal"
                } else {
                    "Start"
                };
                let card_id = card.clone();
                rows = rows.child(action(
                    format!("@{} · {state}", profile.id),
                    Variant::Secondary,
                    move |_| {
                        dispatch_then(
                            ui,
                            "agent.open",
                            json!({"card_id":card_id,"profile_id":profile.id}),
                            move |v| {
                                if let Some(id) = v["request_id"].as_str() {
                                    ui.request.set(id.into());
                                    ui.page.set("terminal".into());
                                    ui.expanded.set(false);
                                    ui.focused_card.set(false);
                                }
                            },
                        )
                    },
                ));
            }
            rows
        }))
}
pub fn execution(ui: Ui, request: Request) -> impl IntoElement {
    run_panel(ui, request)
}
pub fn screen(ui: Ui) -> impl IntoElement {
    let terminal_run = computed(move || {
        let request = ui.request.get();
        ui.snapshot.with(|s| {
            s.attempts
                .iter()
                .rev()
                .find(|a| a.request_id == request && a.terminal.is_some())
                .cloned()
        })
    });
    view()
        .flex_1()
        .min_h(px(0.0))
        .p_4()
        .gap_3()
        .child(dynamic(move || {
            let request_id = ui.request.get();
            let snapshot = ui.snapshot.get();
            let Some(request) = snapshot.requests.iter().find(|r| r.id == request_id) else {
                return label("Select a Card Agent to open its terminal").into_element();
            };
            let card_id = request.card_id.clone();
            let profile_id = request.profile.id.clone();
            let run = snapshot
                .attempts
                .iter()
                .rev()
                .find(|a| a.request_id == request.id);
            let mut toolbar = view()
                .gap_2()
                .child(heading(format!("@{}", profile_id)))
                .child(muted(format!(
                    "{} · {} · {}",
                    request.stage,
                    request.state,
                    run.map(|a| format!("{} · {}", a.close_state, a.connection_state))
                        .unwrap_or_default()
                )))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .child(action("Open Card", Variant::Quiet, {
                            let card_id = card_id.clone();
                            move |_| ui.open_card(card_id.clone())
                        }))
                        .child(action("Open / resume conversation", Variant::Primary, {
                            let card_id = card_id.clone();
                            let profile_id = profile_id.clone();
                            move |_| {
                                dispatch_then(
                                    ui,
                                    "agent.open",
                                    json!({"card_id":card_id,"profile_id":profile_id}),
                                    move |v| {
                                        if let Some(id) = v["request_id"].as_str() {
                                            ui.request.set(id.into());
                                        }
                                    },
                                )
                            }
                        })),
                );
            if let Some(run) = run {
                let interrupt = run.id.clone();
                let end = run.id.clone();
                toolbar = toolbar.child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .child(action("Interrupt", Variant::Secondary, move |_| {
                            dispatch(ui, "agent.interrupt", json!({"attempt_id":interrupt}))
                        }))
                        .child(action("End session", Variant::Danger, move |_| {
                            dispatch(ui, "agent.end", json!({"attempt_id":end}))
                        })),
                );
                if run
                    .observed
                    .as_ref()
                    .is_some_and(|v| v["state"] == "finished")
                {
                    let card_id = card_id.clone();
                    let profile_id = profile_id.clone();
                    toolbar = toolbar.child(action(
                        "Start with retained Card context",
                        Variant::Secondary,
                        move |_| {
                            dispatch_then(
                                ui,
                                "agent.open",
                                json!({"card_id":card_id,"profile_id":profile_id,"fresh":true}),
                                move |v| {
                                    if let Some(id) = v["request_id"].as_str() {
                                        ui.request.set(id.into());
                                    }
                                },
                            )
                        },
                    ));
                }
                if let Some(error) = &run.error {
                    toolbar = toolbar.child(label(error.clone()));
                }
            }
            toolbar.into_element()
        }))
        .child(dynamic(move || {
            let current_run = terminal_run.get();
            let Some(run) = current_run.as_ref() else {
                return panel()
                    .child(label("Waiting for the Agent terminal…"))
                    .into_element();
            };
            let Some(app) = crate::APP.get() else {
                return label("Terminal binding requires a running application").into_element();
            };
            let binding =
                json!({"config":app.config(),"terminal":run.terminal,"core_id":run.core_id});
            native("canban/terminal")
                .key(run.id.clone())
                .prop("binding", binding)
                .prop(
                    "read_only",
                    json!(matches!(
                        run.close_state.as_str(),
                        "pending" | "closing" | "closed"
                    )),
                )
                .flex_1()
                .min_h(px(200.0))
                .into_element()
        }))
}
