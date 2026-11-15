pub mod activity;
pub mod git_workspace;
pub mod scheduling;
pub use activity::{activity_header, empty};
pub(crate) mod card_tile;
pub mod timeline_item;
use crate::presenter::*;
use canban_types::Event;
use canban_types::*;
use canban_ui_kit::{tokens::*, *};
pub(crate) use card_tile::card_terminal_icon_tint;
pub use card_tile::card_tile;
use nsh_core::prelude::*;
use serde_json::json;
pub use timeline_item::timeline_item;
pub fn run_panel(ui: Ui, entry: TerminalEntry) -> impl IntoElement {
    let run = entry.agent.expect("run_panel requires an Agent terminal");
    let id = entry.id;
    let mut panel = panel()
        .child(label(format!("@{} · {}", run.profile.id, run.state)))
        .child(muted(format!("{} · {}", run.stage, run.trigger)));
    panel = panel.child(read_text(run.instruction.clone()));
    let current = crate::terminals::model::conversations(&ui.snapshot.get_untracked())
        .into_iter()
        .find(|r| {
            r.card.id == run.card_id && r.agent().is_some_and(|a| a.profile.id == run.profile.id)
        });
    let (card_active, is_deliver_agent, already_queued_for_merge) =
        ui.snapshot.with_untracked(|s| {
            let active = s.cards.iter().any(|c| c.id == run.card_id && c.active());
            let deliver_run = s.workflow_runs.iter().find(|r| {
                r.card_id == run.card_id
                    && r.active()
                    && r.step_is_deliver()
                    && (r.terminal_id.as_deref() == Some(id.as_str())
                        || run.workflow_run_id.as_deref() == Some(r.id.as_str()))
            });
            (
                active,
                deliver_run.is_some(),
                deliver_run.is_some_and(|r| r.state == "waiting_delivery"),
            )
        });
    let can_retry = current.as_ref().is_none_or(crate::terminals::model::closed)
        && card_active
        && !already_queued_for_merge;
    if let Some(row) = current {
        if crate::terminals::model::binding(&row).is_some() {
            let row_id = row.id.clone();
            let card_id = run.card_id.clone();
            panel = panel.child(action(
                if row.id == id {
                    "Open Terminal"
                } else {
                    "Open current conversation"
                },
                Variant::Secondary,
                move |_| {
                    ui.open_card_with_terminal(card_id.clone(), Some(row_id.clone()));
                },
            ));
        } else if crate::terminals::model::can_resume(&row, &ui.profiles.get_untracked()) {
            let row_id = row.id.clone();
            panel = panel.child(
                action("Resume Agent", Variant::Secondary, move |_| {
                    crate::terminals::navigation::resume(ui, &row_id)
                })
                .disabled(move || ui.pending.get()),
            );
        }
    }
    let output_id = id.clone();
    let host_id = ui.snapshot.with(|s| s.card_host(&run.card_id).to_owned());
    panel = panel
        .child(muted(format!(
            "Run {} · {} · {}",
            run.number,
            run.terminal.as_deref().unwrap_or("no terminal"),
            run.cwd
        )))
        .child(muted(format!(
            "Host: {}",
            crate::hosts::host_name(ui, &host_id)
        )))
        .child(action(
            "Read terminal output",
            Variant::Secondary,
            move |_| load_text(ui, "execution.output", json!({"terminal_id":output_id})),
        ));
    if let Some(error) = run.error {
        panel = panel.child(label(error).text_color(DANGER));
    }
    if can_retry
        && matches!(
            run.state.as_str(),
            "failed" | "blocked" | "needs_report" | "missing"
        )
    {
        let retry_id = id.clone();
        panel = panel.child(
            action(
                if is_deliver_agent {
                    "Retry auto-merge"
                } else {
                    "Retry with a fresh Session"
                },
                Variant::Secondary,
                move |_| dispatch(ui, "execution.retry", json!({"terminal_id":retry_id})),
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
