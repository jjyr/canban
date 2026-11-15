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
                .terminals
                .iter()
                .filter(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.state == "queued" || a.pending.is_some())
                })
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
    let tracked_host = signal(ui.settings_host.get_untracked());
    effect(move || {
        let cur_host = ui.settings_host.get();
        let max_v = ui.max_agents.get();
        let idle_v = ui.idle_minutes.get();
        if tracked_host.get_untracked() != cur_host {
            tracked_host.set(cur_host);
            value.set(max_v.to_string());
            idle.set(idle_v.to_string());
            error.set(String::new());
        }
    });
    settings_section("Agent scheduling", "Shared execution limits and idle timeout across all Projects.")
        .child(summary(ui))
        .child(settings_field("Maximum executing Agents", "Lowering the limit leaves running Agents alone. Later workflow stages run first; human review gates stay paused.",
            field(value, "8").label("Maximum executing Agents").max_w(px(160.))))
        .child(settings_field("Idle timeout (minutes)", "Idle Agents remain interactive during this interval; human input restarts the timer. Closed sessions leave Terminals and can be resumed from their Card. Use 0 to disable.",
            field(idle, "15").label("Idle timeout (minutes)").max_w(px(160.))))
        .child(view().items_start()
        .child(action("Save scheduling settings", Variant::Primary, move |_| {
            match value.get_untracked().parse::<usize>() {
                Ok(max) if max > 0 => {
                    let Ok(minutes) = idle.get_untracked().parse::<u64>() else {error.set("Idle minutes must be a nonnegative whole number".into());return;};
                    dispatch_complete(ui, "scheduler.configure", json!({"max_agents":max,"idle_minutes":minutes}), move |result| {
                        match result { Ok(_) => { ui.max_agents.set(max); ui.idle_minutes.set(minutes); error.try_set(String::new()); }, Err(e) => { error.try_set(e); } }
                    });
                },
                _ => error.set("Enter a positive whole number".into()),
            }
        }).disabled(move || ui.pending.get())))
        .child(dynamic(move || if error.get().is_empty() { view().into_element() } else { notice(error.get(), Tone::Danger).into_element() }))
}
