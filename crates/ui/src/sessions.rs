//! One reusable Card/profile entry and native terminal screen.
use crate::presenter::*;
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
                .child(chrome_button(
                    Icon::Terminal,
                    "Open Shell in Card directory",
                    move |_| {
                        if let Some(card) = ui.current_card() {
                            crate::terminals::create(ui, card.project_id, Some(card.id));
                        }
                    },
                ))
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
            let conversations = crate::terminals::model::conversations(&snapshot);
            for profile in profiles {
                let row = conversations
                    .iter()
                    .find(|r| r.id == canban_types::Conversation::key(&card, &profile.id));
                let latest = row.and_then(|r| r.request.as_ref());
                let run = row.and_then(|r| r.attempt.as_ref());
                let exited = run.is_some_and(|a| {
                    a.observed
                        .as_ref()
                        .is_some_and(|s| s["state"] == "finished")
                });
                let state = if exited {
                    "Resume"
                } else if latest.is_some() {
                    "Open terminal"
                } else {
                    "Start"
                };
                let card_id = card.clone();
                rows = rows.child(action(
                    format!("@{} · {state}", profile.id),
                    Variant::Secondary,
                    move |_| {
                        open(ui, card_id.clone(), profile.id.clone());
                    },
                ));
            }
            rows
        }))
}

/// Explicit Start/Resume intent shared by Card components. A late operation
/// receipt updates saved state without redirecting a user who has navigated away.
pub fn open(ui: Ui, card: String, profile: String) {
    let origin = ui.location();
    let conversation = canban_types::Conversation::key(&card, &profile);
    dispatch_then(
        ui,
        "agent.open",
        json!({"card_id":card,"profile_id":profile}),
        move |v| {
            if v["request_id"].is_string() && ui.location() == origin {
                ui.open_terminal(conversation);
            }
        },
    );
}
