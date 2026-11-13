pub mod activity;
pub mod git_workspace;
pub mod scheduling;
pub use activity::{activity_header, empty};
mod card_tile;
pub(crate) mod timeline_item;
use crate::presenter::*;
use canban_types::Event;
use canban_types::*;
use canban_ui_kit::{tokens::*, *};
pub use card_tile::card_tile;
use nsh_core::prelude::*;
use serde_json::json;
pub use timeline_item::timeline_item;
pub fn run_panel(ui: Ui, req: Request) -> impl IntoElement {
    let run = ui.snapshot.with_untracked(|s| {
        s.attempts
            .iter()
            .rev()
            .find(|a| a.request_id == req.id)
            .cloned()
    });
    let mut panel = panel()
        .child(label(format!("@{} · {}", req.profile.id, req.state)))
        .child(muted(format!("{} · {}", req.stage, req.trigger)));
    panel = panel.child(read_text(req.instruction.clone()));
    let current = crate::terminals::model::conversations(&ui.snapshot.get_untracked())
        .into_iter()
        .find(|r| {
            r.card.id == req.card_id
                && r.request
                    .as_ref()
                    .is_some_and(|r| r.profile.id == req.profile.id)
        });
    let can_retry = current.as_ref().is_none_or(crate::terminals::model::closed)
        && ui
            .snapshot
            .with_untracked(|s| s.cards.iter().any(|c| c.id == req.card_id && c.active()));
    if let Some(row) = current {
        if crate::terminals::model::binding(&row).is_some() {
            let id = row.id.clone();
            let card_id = req.card_id.clone();
            panel = panel.child(action(
                if row.request.as_ref().is_some_and(|r| r.id == req.id) {
                    "Open Terminal"
                } else {
                    "Open current conversation"
                },
                Variant::Secondary,
                move |_| {
                    ui.open_card_with_terminal(card_id.clone(), Some(id.clone()));
                },
            ));
        } else if crate::terminals::model::can_resume(&row, &ui.profiles.get_untracked()) {
            let id = row.id.clone();
            panel = panel.child(
                action("Resume Agent", Variant::Secondary, move |_| {
                    crate::terminals::navigation::resume(ui, &id)
                })
                .disabled(move || ui.pending.get()),
            );
        }
    }
    if let Some(run) = run {
        let id = run.id.clone();
        panel = panel
            .child(muted(format!(
                "Attempt {} · {} · {}",
                run.number,
                run.terminal.as_deref().unwrap_or("no terminal"),
                run.cwd
            )))
            .child(muted(format!(
                "Machine: {}",
                crate::machines::name(
                    ui,
                    run.location
                        .as_ref()
                        .map(|l| l.machine.as_str())
                        .unwrap_or("local")
                )
            )))
            .child(action(
                "Read terminal output",
                Variant::Secondary,
                move |_| load_text(ui, "execution.output", json!({"attempt_id":id})),
            ));
        if let Some(error) = run.error {
            panel = panel.child(label(error).text_color(DANGER));
        }
    }
    if can_retry && matches!(req.state.as_str(), "failed" | "blocked" | "needs_report") {
        let id = req.id.clone();
        panel = panel.child(
            action(
                "Retry with a fresh Session",
                Variant::Secondary,
                move |_| dispatch(ui, "execution.retry", json!({"request_id":id})),
            )
            .disabled(move || ui.pending.get()),
        );
    }
    panel
}
pub fn output_panel(ui: Ui) -> impl IntoElement {
    panel()
        .child(
            view()
                .flex_row()
                .justify_between()
                .child(label("Output / report"))
                .child(action("Clear", Variant::Quiet, move |_| {
                    ui.output_request.set(ui.output_request.get_untracked() + 1);
                    ui.output.set(String::new());
                })),
        )
        .child(nsh_base::text_view().markdown(move || Some(ui.output.get().into())))
}
pub mod workflow;
