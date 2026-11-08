//! Global grouped Agent list + context inspector. Selection has no execution side effects.
//! Stable conversation keys, retained scroll and explicit destination actions.
use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_notifications::{GROUPS, OverviewRow, overview};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

fn select(ui: Ui, row: OverviewRow) {
    ui.clear_output();
    ui.overview_history.set(false);
    ui.overview_selection.set(row.id.clone());
    ui.overview_selected.set(Some(row));
}
fn destination(ui: Ui, row: &OverviewRow, page: &str) {
    ui.return_overview.set(true);
    ui.open_card(row.card.id.clone());
    if let Some(req) = &row.request {
        ui.request.set(req.id.clone());
    }
    ui.page.set(page.into());
}
pub fn screen(ui: Ui) -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .p_4()
        .gap_3()
        .child(view().gap_1().child(heading("Overview")).child(muted(
            "All Projects · Agents, workflow decisions and recovery",
        )))
        .child(crate::components::scheduling::summary(ui))
        .child(dynamic(move || {
            let selected = !ui.overview_selection.get().is_empty();
            let narrow = ui.narrow.get();
            untracked(|| {
                if selected {
                    if narrow {
                        inspector(ui).into_element()
                    } else {
                        split(
                            list(ui).into_element(),
                            inspector(ui).into_element(),
                            ui.overview_width,
                            350.0,
                            340.0,
                        )
                        .into_element()
                    }
                } else {
                    list(ui).into_element()
                }
            })
        }))
}
fn list(ui: Ui) -> impl IntoElement {
    scroll()
        .flex_1()
        .min_h(px(0.0))
        .content_offset(scroll_position(ui, "overview:list"))
        .on_scroll(move |e: &ScrollEvent| capture_scroll(ui, "overview:list".into(), e))
        .child(dynamic(move || {
            let rows = overview(&ui.snapshot.get());
            let selected = ui.overview_selection.get();
            let closed = ui.overview_closed.get();
            let mut content = view().gap_4();
            if rows.is_empty() {
                return content.child(crate::components::activity::empty(
                    "No Agents here",
                    "Move a Card into an automated stage or @ an Agent on a Card.",
                ));
            }
            for group in GROUPS {
                let members = rows.iter().filter(|r| r.group == group).collect::<Vec<_>>();
                if members.is_empty() {
                    continue;
                }
                let mut section = view().gap_2();
                if group == "Closed" {
                    section = section.child(action(
                        format!(
                            "{} Closed · {}",
                            if closed { "▾" } else { "▸" },
                            members.len()
                        ),
                        Variant::Quiet,
                        move |_| ui.overview_closed.set(!ui.overview_closed.get_untracked()),
                    ));
                    if !closed {
                        content = content.child(section);
                        continue;
                    }
                } else {
                    section = section.child(status(
                        format!(
                            "{} · {}",
                            if group == "Needs attention" {
                                "Needs you"
                            } else {
                                group
                            },
                            members.len()
                        ),
                        if group == "Needs attention" {
                            Tone::Attention
                        } else if group == "Ready for review" {
                            Tone::Info
                        } else {
                            Tone::Neutral
                        },
                    ));
                }
                for row in members {
                    section = section.child(agent_row(ui, row.clone(), row.id == selected));
                }
                content = content.child(section);
            }
            content
        }))
}
/// Reused by Gallery; uses production semantic tokens and NSH button semantics.
pub fn agent_row(ui: Ui, row: OverviewRow, selected: bool) -> impl IntoElement {
    let profile = row
        .request
        .as_ref()
        .map(|r| format!("@{}", r.profile.id))
        .unwrap_or_else(|| "Card recovery".into());
    let reason = row
        .reasons
        .first()
        .map(|r| r.message.clone())
        .unwrap_or_else(|| row.group.clone());
    let since = row
        .attempt
        .as_ref()
        .and_then(|a| {
            a.lifecycle
                .waiting
                .as_ref()
                .map(|w| w.since)
                .or(a.lifecycle.idle_since)
        })
        .unwrap_or_else(|| {
            row.request
                .as_ref()
                .map(|r| r.created_at)
                .unwrap_or(row.card.created_at)
        });
    nsh_base::button()
        .w_full()
        .items_start()
        .p_3()
        .gap_1()
        .rounded(px(RADIUS))
        .bg(if selected { SELECTED } else { SURFACE })
        .border_1()
        .border_color(if selected {
            ACCENT
        } else if row.actionable() {
            ATTENTION
        } else {
            BORDER
        })
        .hover(|s| s.bg(INSET))
        .focus(|s| s.border_color(ACCENT))
        .child(
            view()
                .w_full()
                .flex_row()
                .justify_between()
                .gap_2()
                .child(label(format!(
                    "{} · #{} {}",
                    profile, row.card.number, row.card.title
                )))
                .child(muted(crate::components::activity::age(since))),
        )
        .child(muted(format!(
            "{} / {} · {}",
            row.project_name, row.card.stage, reason
        )))
        .child(muted(format!(
            "Branch · {}",
            row.card.branch.as_deref().unwrap_or("Not created")
        )))
        .on_press(move |_: &()| select(ui, row.clone()))
}
fn inspector(ui: Ui) -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .gap_2()
        .child(action(
            if ui.narrow.get_untracked() {
                "← Overview"
            } else {
                "Close context"
            },
            Variant::Quiet,
            move |_| {
                ui.clear_output();
                ui.overview_selection.set(String::new());
            },
        ))
        .child(dynamic(move || row_actions(ui)))
        .child(scroll().flex_1().min_h(px(0.0)).child(dynamic(move || {
            let snapshot = ui.snapshot.get();
            let id = ui.overview_selection.get();
            let rows = overview(&snapshot);
            let row = rows
                .into_iter()
                .find(|r| r.id == id)
                .or_else(|| ui.overview_selected.get());
            let Some(row) = row else {
                return view()
                    .child(muted("Select an Agent to inspect its context"))
                    .into_element();
            };
            let mut body = panel()
                .gap_3()
                .child(heading(format!("#{} {}", row.card.number, row.card.title)))
                .child(muted(format!(
                    "{} / {} · {}",
                    row.project_name, row.card.stage, row.group
                )));
            for reason in &row.reasons {
                body = body.child(notice(
                    format!(
                        "{} · {}",
                        match reason.source.as_str() {
                            "task" => "Task decision",
                            "runtime" => "Agent input",
                            "review" => "Review",
                            "recovery" => "Recovery",
                            _ => "Question",
                        },
                        reason.message
                    ),
                    if reason.source == "review" {
                        Tone::Info
                    } else {
                        Tone::Attention
                    },
                ));
            }
            if let Some(req) = &row.request {
                body = body
                    .child(label(format!(
                        "@{} · {} · {}",
                        req.profile.id,
                        req.profile.command_name(),
                        req.state
                    )))
                    .child(label("Assignment"))
                    .child(read_text(
                        req.instruction.chars().take(16_384).collect::<String>(),
                    ));
                for q in snapshot.questions.iter().filter(|q| {
                    q.card_id == row.card.id
                        && q.profile_id == req.profile.id
                        && q.state != "replied"
                }) {
                    body = body
                        .child(label(format!("@question · {}", q.state)))
                        .child(read_text(q.body.clone()));
                }
                if let Some(report) = snapshot
                    .reports
                    .iter()
                    .rev()
                    .find(|r| Some(r.request_id.as_str()) == row.outcome_request_id())
                {
                    body = body
                        .child(label("Latest saved outcome"))
                        .child(read_text(format!("{} · {}", report.state, report.message)));
                    let request_id = report.request_id.clone();
                    body = body.child(action("Read saved report", Variant::Quiet, move |_| {
                        load_text(ui, "execution.result", json!({"request_id":request_id}))
                    }));
                }
                if let Some(a) = &row.attempt {
                    body = body.child(label("Herdr runtime context"));
                    body = body.child(muted(format!(
                        "{} · {}",
                        crate::machines::name(
                            ui,
                            a.location
                                .as_ref()
                                .map(|l| l.machine.as_str())
                                .unwrap_or("local")
                        ),
                        a.cwd
                    )));
                    if let Some(v) = &a.observed {
                        body = body.child(read_text(format!(
                            "{}\n{}",
                            v["status"]["title"]
                                .as_str()
                                .unwrap_or("No structured title"),
                            v["status"]["msg"].as_str().unwrap_or("")
                        )));
                    }
                    body = body.child(muted(format!(
                        "{} · {} · last observed {}",
                        a.connection_state,
                        a.terminal.as_deref().unwrap_or("Unbound"),
                        a.lifecycle
                            .observed_at
                            .map(crate::components::activity::age)
                            .unwrap_or_else(|| "never".into())
                    )));
                    let row_id = row.id.clone();
                    let attempt_id = a.id.clone();
                    body = body.child(action("Read terminal preview", Variant::Quiet, move |_| {
                        load_text(
                            ui,
                            "overview.preview",
                            json!({"row_id":row_id,"attempt_id":attempt_id}),
                        )
                    }));
                }
                let history = row
                    .history
                    .iter()
                    .filter_map(|id| snapshot.attempts.iter().find(|a| &a.id == id))
                    .map(|a| {
                        format!(
                            "Attempt {} · {} · {}",
                            a.number,
                            a.state,
                            a.terminal.as_deref().unwrap_or("Unbound")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                body = body.child(action(
                    format!(
                        "{} Invocation history · {}",
                        if ui.overview_history.get() {
                            "▾"
                        } else {
                            "▸"
                        },
                        row.history.len()
                    ),
                    Variant::Quiet,
                    move |_| {
                        ui.overview_history
                            .set(!ui.overview_history.get_untracked())
                    },
                ));
                if ui.overview_history.get() {
                    body = body.child(read_text(history));
                }
            }
            body = body.child(muted(format!(
                "Branch: {}\nWorkspace: {}",
                row.card.branch.as_deref().unwrap_or("None"),
                row.card.worktree.as_deref().unwrap_or("Not prepared")
            )));
            body.into_element()
        })))
        .child(dynamic(move || {
            let output = ui.output.get();
            if output.is_empty() {
                view().into_element()
            } else {
                panel()
                    .max_h(px(260.0))
                    .child(label("Read-only context · loaded on request"))
                    .child(muted(move || {
                        let _ = ui.snapshot.get();
                        ui.output_observed_at
                            .get()
                            .map(|at| {
                                format!(
                                    "Herdr snapshot captured {} · refresh to update",
                                    crate::components::activity::age(at)
                                )
                            })
                            .unwrap_or_default()
                    }))
                    .child(scroll().child(read_text(output)))
                    .into_element()
            }
        }))
}

fn row_actions(ui: Ui) -> impl IntoElement {
    let id = ui.overview_selection.get();
    let row = overview(&ui.snapshot.get())
        .into_iter()
        .find(|r| r.id == id)
        .or_else(|| ui.overview_selected.get());
    let mut actions = view().flex_row().flex_wrap().gap_2();
    let Some(row) = row else {
        return actions;
    };
    let card_row = row.clone();
    actions = actions.child(action("Open Card", Variant::Secondary, move |_| {
        destination(ui, &card_row, "card")
    }));
    if row.reasons.iter().any(|r| r.source == "review") {
        let review_row = row.clone();
        actions = actions.child(action("Review result", Variant::Primary, move |_| {
            destination(ui, &review_row, "review")
        }));
    }
    if let (Some(req), Some(a)) = (&row.request, &row.attempt) {
        if !a.finished() && a.terminal.is_some() {
            let terminal_row = row.clone();
            actions = actions.child(action("Open terminal", Variant::Primary, move |_| {
                ui.open_terminal(terminal_row.id.clone())
            }));
        } else if a.finished() && row.card.archived_at.is_none() {
            let resume_row = row.clone();
            let attempt_id = a.id.clone();
            let card_id = row.card.id.clone();
            let profile = req.profile.id.clone();
            let can_resume = a.provider_session_id.is_some()
                && ui.profiles.with(|ps| {
                    ps.iter()
                        .any(|p| p.id == profile && !p.resume_command().is_empty())
                });
            actions=actions.child(action("Resume exact session",Variant::Secondary,move |_| {
                let origin=ui.location();
                let saved=resume_row.clone();
                dispatch_then(ui,"agent.open",json!({"card_id":card_id,"profile_id":profile,"expected_attempt_id":attempt_id}),move |v| {
                    if v["request_id"].is_string() && ui.location()==origin { ui.open_terminal(saved.id); }
                });
            }).disabled(!can_resume));
            if !can_resume {
                actions = actions.child(muted("Exact resume unavailable · open Card for recovery"));
            }
        }
    }
    actions
}
