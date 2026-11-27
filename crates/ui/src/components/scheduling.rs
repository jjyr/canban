//! Shared global admission summary and auto-saving scheduling settings, also used in the Gallery.
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

pub fn summary(ui: Ui) -> impl IntoElement {
    muted(move || {
        let max_agents = ui.max_agents.get();
        let (active, queued) = ui.snapshot.with(|snapshot| {
            (
                crate::backend::active_agent_count(snapshot),
                snapshot
                    .terminals
                    .iter()
                    .filter(|t| {
                        t.agent
                            .as_ref()
                            .is_some_and(|a| a.state == "queued" || a.pending.is_some())
                    })
                    .count(),
            )
        });
        let (idle, need_you) = ui.overview_rows.with(|rows| {
            (
                rows.iter().filter(|r| r.group == "Idle").count(),
                rows.iter().filter(|r| r.actionable()).count(),
            )
        });
        format!(
            "Execution slots {active} / {max_agents} · {queued} queued · {idle} idle · {need_you} need you",
        )
    })
}

pub fn settings(ui: Ui) -> impl IntoElement {
    let max_draft = signal(ui.max_agents.get_untracked().to_string());
    let idle_draft = signal(ui.idle_minutes.get_untracked().to_string());
    let error = signal(String::new());
    let tracked_host = stored(ui.settings_host.get_untracked());
    let last_synced = stored((
        ui.max_agents.get_untracked(),
        ui.idle_minutes.get_untracked(),
    ));

    effect(move || {
        let cur_host = ui.settings_host.get();
        let max_v = ui.max_agents.get();
        let idle_v = ui.idle_minutes.get();
        if tracked_host.get() != cur_host || last_synced.get() != (max_v, idle_v) {
            tracked_host.replace(cur_host);
            last_synced.replace((max_v, idle_v));
            max_draft.set(max_v.to_string());
            idle_draft.set(idle_v.to_string());
            error.set(String::new());
        }
    });

    let commit = move |next_max: usize, next_idle: u64| {
        if next_max == 0 {
            error.set("Maximum executing Agents must be a positive whole number.".into());
            return;
        }
        max_draft.set(next_max.to_string());
        idle_draft.set(next_idle.to_string());
        error.set(String::new());
        if next_max == ui.max_agents.get_untracked() && next_idle == ui.idle_minutes.get_untracked()
        {
            return;
        }
        if !crate::backend::is_connected() {
            last_synced.replace((next_max, next_idle));
            ui.max_agents.set(next_max);
            ui.idle_minutes.set(next_idle);
            return;
        }
        if ui.is_method_pending("scheduler.configure") {
            return;
        }
        let prev_max = ui.max_agents.get_untracked();
        let prev_idle = ui.idle_minutes.get_untracked();
        last_synced.replace((next_max, next_idle));
        ui.max_agents.set(next_max);
        ui.idle_minutes.set(next_idle);
        dispatch_complete(
            ui,
            "scheduler.configure",
            json!({ "max_agents": next_max, "idle_minutes": next_idle }),
            move |result| match result {
                Ok(_) => {
                    let _ = error.try_set(String::new());
                }
                Err(e) => {
                    last_synced.replace((prev_max, prev_idle));
                    ui.max_agents.set(prev_max);
                    ui.idle_minutes.set(prev_idle);
                    let _ = max_draft.try_set(prev_max.to_string());
                    let _ = idle_draft.try_set(prev_idle.to_string());
                    let _ = error.try_set(e);
                }
            },
        );
    };

    let commit_drafts = move || {
        let max_raw = max_draft.get_untracked();
        let idle_raw = idle_draft.get_untracked();
        let Ok(max) = max_raw.trim().parse::<usize>() else {
            error.set("Enter a positive whole number for maximum executing Agents.".into());
            return;
        };
        if max == 0 {
            error.set("Maximum executing Agents must be at least 1.".into());
            return;
        }
        let Ok(minutes) = idle_raw.trim().parse::<u64>() else {
            error.set("Idle minutes must be a nonnegative whole number.".into());
            return;
        };
        commit(max, minutes);
    };

    let step_max = move |delta: isize| {
        let cur = max_draft
            .get_untracked()
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or_else(|| ui.max_agents.get_untracked());
        let next = (cur as isize + delta).max(1) as usize;
        let idle_val = idle_draft
            .get_untracked()
            .trim()
            .parse::<u64>()
            .unwrap_or_else(|_| ui.idle_minutes.get_untracked());
        commit(next, idle_val);
    };

    let step_idle = move |delta: i64| {
        let cur = idle_draft
            .get_untracked()
            .trim()
            .parse::<u64>()
            .unwrap_or_else(|_| ui.idle_minutes.get_untracked());
        let next = (cur as i64 + delta).max(0) as u64;
        let max_val = max_draft
            .get_untracked()
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or_else(|| ui.max_agents.get_untracked());
        commit(max_val, next);
    };

    let stepper_btn = |symbol: &'static str,
                       a11y: &'static str,
                       border_left: bool,
                       disabled: Box<dyn Fn() -> bool>,
                       on_press: Box<dyn Fn(&())>| {
        let mut btn = press_target()
            .w(px(28.))
            .h(px(30.))
            .items_center()
            .justify_center()
            .hover(|s| s.bg(SURFACE))
            .active(|s| s.opacity(0.6))
            .focus(|s| s.bg(SELECTED))
            .disabled(disabled)
            .disabled_style(|s| s.opacity(0.4))
            .accessibility_label(a11y)
            .child(
                label(symbol)
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM),
            )
            .on_press(on_press);
        btn = if border_left {
            btn.border_l_1().border_color(BORDER)
        } else {
            btn.border_r_1().border_color(BORDER)
        };
        btn
    };

    let max_stepper = view()
        .flex_row()
        .items_center()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(INSET)
        .overflow_hidden()
        .child(stepper_btn(
            "−",
            "Decrease maximum executing Agents",
            false,
            Box::new(move || {
                ui.is_method_pending("scheduler.configure")
                    || max_draft
                        .get()
                        .trim()
                        .parse::<usize>()
                        .map(|n| n <= 1)
                        .unwrap_or(false)
            }),
            Box::new(move |_| step_max(-1)),
        ))
        .child(
            view().w(px(54.)).bg(SURFACE).child(
                cell_field(
                    move || max_draft.get(),
                    move |v| {
                        error.set(String::new());
                        max_draft.set(v.to_owned());
                    },
                    "8",
                )
                .font_family(MONO_FONT)
                .label("Maximum executing Agents")
                .on_submit(move |_: &str| commit_drafts())
                .on_blur(commit_drafts),
            ),
        )
        .child(stepper_btn(
            "+",
            "Increase maximum executing Agents",
            true,
            Box::new(move || ui.is_method_pending("scheduler.configure")),
            Box::new(move |_| step_max(1)),
        ));

    let idle_stepper = view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(BORDER)
                .bg(INSET)
                .overflow_hidden()
                .child(stepper_btn(
                    "−",
                    "Decrease idle timeout minutes",
                    false,
                    Box::new(move || {
                        ui.is_method_pending("scheduler.configure")
                            || idle_draft
                                .get()
                                .trim()
                                .parse::<u64>()
                                .map(|n| n == 0)
                                .unwrap_or(false)
                    }),
                    Box::new(move |_| step_idle(-5)),
                ))
                .child(
                    view().w(px(54.)).bg(SURFACE).child(
                        cell_field(
                            move || idle_draft.get(),
                            move |v| {
                                error.set(String::new());
                                idle_draft.set(v.to_owned());
                            },
                            "15",
                        )
                        .font_family(MONO_FONT)
                        .label("Idle timeout (minutes)")
                        .on_submit(move |_: &str| commit_drafts())
                        .on_blur(commit_drafts),
                    ),
                )
                .child(stepper_btn(
                    "+",
                    "Increase idle timeout minutes",
                    true,
                    Box::new(move || ui.is_method_pending("scheduler.configure")),
                    Box::new(move |_| step_idle(5)),
                )),
        )
        .child(
            muted(move || {
                if idle_draft.get().trim() == "0" {
                    "disabled"
                } else {
                    "min"
                }
            })
            .text_size(px(CAPTION_SIZE)),
        );

    settings_section(
        "Agent Scheduling",
        "Shared execution limits and idle timeout across all Projects on this host. Changes save automatically.",
    )
    .trailing(when(
        move || ui.is_method_pending("scheduler.configure"),
        || status("Saving…", Tone::Info),
    ))
    .child(summary(ui))
    .child(
        settings_group()
            .row(settings_row(
                "Maximum executing Agents",
                "Lowering the limit leaves running Agents alone. Later workflow stages run first; human review gates stay paused.",
                max_stepper,
            ))
            .row(settings_row(
                "Idle timeout (minutes)",
                "Interactive Agents stay open during this interval; completed tasks reclaim after 30s of verified idle, and human input restarts the timer. Use 0 to disable.",
                idle_stepper,
            )),
    )
    .child(when(
        move || !error.get().is_empty(),
        move || notice(move || error.get(), Tone::Danger),
    ))
}
