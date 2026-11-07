//! Shared global admission summary and settings, also used in the Gallery.
use crate::presenter::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::json;
pub fn summary(ui: Ui) -> impl IntoElement {
    muted(move || {
        let snapshot = ui.snapshot.get();
        let rows = canban_notifications::overview(&snapshot);
        format!(
            "Execution slots {} / {} · {} queued · {} idle · {} need you",
            canban_scheduler::active_count(&snapshot),
            ui.max_agents.get(),
            snapshot
                .requests
                .iter()
                .filter(|r| r.state == "queued")
                .count(),
            rows.iter().filter(|r| r.group == "Idle").count(),
            rows.iter().filter(|r| r.actionable()).count(),
        )
    })
}
pub fn settings(ui: Ui) -> impl IntoElement {
    let value = signal(ui.max_agents.get_untracked().to_string());
    let error = signal(String::new());
    let idle = signal(ui.idle_minutes.get_untracked().to_string());
    panel().gap_2().child(heading("Agent scheduling")).child(summary(ui))
        .child(label("Maximum executing Agents"))
        .child(field(value, "8"))
        .child(muted("Shared across all Boards. Lowering the limit leaves running Agents alone. Later workflow stages run first; Cards waiting for human review stay paused."))
        .child(label("Idle close after minutes · 0 disables"))
        .child(field(idle,"15"))
        .child(muted("Verified waits and idle Agents release scheduling slots. Resumable idle conversations close normally after this timeout; viewed terminals and unresolved work remain open."))
        .child(action("Save Agent limit", Variant::Primary, move |_| {
            match value.get_untracked().parse::<usize>() {
                Ok(max) if max > 0 => {
                    let Ok(minutes) = idle.get_untracked().parse::<u64>() else {error.set("Idle minutes must be a nonnegative whole number".into());return;};
                    dispatch_complete(ui, "scheduler.configure", json!({"max_agents":max,"idle_minutes":minutes}), move |result| {
                        match result { Ok(_) => { ui.max_agents.set(max); error.try_set(String::new()); }, Err(e) => { error.try_set(e); } }
                    });
                },
                _ => error.set("Enter a positive whole number".into()),
            }
        }).disabled(move || ui.pending.get()))
        .child(dynamic(move || if error.get().is_empty() { view().into_element() } else { notice(error.get(), Tone::Danger).into_element() }))
}
