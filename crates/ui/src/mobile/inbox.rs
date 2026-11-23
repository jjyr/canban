//! `Needs you` mobile inbox tab: actionable approvals/attentions, unread updates, and running agents.
use super::{
    CardSegment, MobileState,
    chrome::{offline_sync_banner, trigger_haptic},
};
use crate::presenter::*;
use canban_types::HerdrStatus;
use canban_ui_kit::{pull_refresh_scroll, tokens::*, *};
use nsh_core::{Haptic, prelude::*};
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Clone, PartialEq)]
struct ActionItem {
    id: String,
    card_id: String,
    project_name: String,
    card_number: u64,
    card_title: String,
    reasons: Vec<String>,
    terminal_id: Option<String>,
    workflow_run: Option<(String, u32, bool)>, // (run_id, visit, is_deliver)
    card_revision: u64,
}

#[derive(Clone, PartialEq)]
struct TerminalAlertItem {
    id: String,
    title: String,
    detail: String,
}

#[derive(Clone, PartialEq)]
struct UpdateItem {
    card_id: String,
    card_number: u64,
    card_title: String,
    project_name: String,
    unread_count: usize,
    last_seq: u64,
}

#[derive(Clone, PartialEq)]
struct RunningItem {
    terminal_id: String,
    card_id: Option<String>,
    title: String,
    subtitle: String,
    started_at: Option<u64>,
    is_blocked: bool,
}

#[derive(Clone, PartialEq, Default)]
struct InboxModel {
    action_items: Vec<ActionItem>,
    terminal_alerts: Vec<TerminalAlertItem>,
    updates: Vec<UpdateItem>,
    running: Vec<RunningItem>,
}

/// Returns total actionable items count driving the `Needs you` bottom tab badge.
pub fn badge_count(ui: Ui) -> usize {
    let all_rows = ui.overview_rows.get();
    let actionable_cards: BTreeSet<&str> = all_rows
        .iter()
        .filter(|r| r.actionable())
        .map(|r| r.card.id.as_str())
        .collect();
    let terminal_alerts = ui.snapshot.with(|_s| {
        ui.terminal_entries.with(|entries| {
            entries
                .iter()
                .filter(|e| {
                    e.agent.is_none()
                        && e.listed()
                        && (e.needs_attention() || e.herdr_status().is_attention())
                })
                .count()
        })
    });
    actionable_cards.len() + terminal_alerts
}

fn build_inbox_model(ui: Ui) -> InboxModel {
    let all_rows = ui.overview_rows.get();
    let entries = ui.terminal_entries.get();
    let counts = ui.reminder_counts.get();

    ui.snapshot.with(|s| {
        let mut seen_cards = BTreeSet::new();
        let mut action_items = Vec::new();
        let mut running = Vec::new();

        for row in all_rows.iter() {
            if row.actionable() && seen_cards.insert(row.card.id.clone()) {
                let wf = s
                    .workflow_runs
                    .iter()
                    .find(|r| {
                        r.active()
                            && r.card_id.as_deref() == Some(row.card.id.as_str())
                            && r.state == "waiting_approval"
                    })
                    .map(|r| {
                        let is_deliver = r.step.as_ref().is_some_and(|k| {
                            matches!(
                                r.definition.steps.get(k),
                                Some(canban_types::WorkflowStep::Deliver { .. })
                            )
                        });
                        (r.id.clone(), r.visit, is_deliver)
                    });
                action_items.push(ActionItem {
                    id: row.id.clone(),
                    card_id: row.card.id.clone(),
                    project_name: row.project_name.clone(),
                    card_number: row.card.number,
                    card_title: row.card.title.clone(),
                    reasons: row.reasons.iter().map(|r| r.message.clone()).collect(),
                    terminal_id: row.terminal.as_ref().map(|t| t.id.clone()),
                    workflow_run: wf,
                    card_revision: row.card.revision,
                });
            } else if row.group == "Working" && !row.actionable() {
                let terminal_id = row
                    .terminal
                    .as_ref()
                    .map(|t| t.id.clone())
                    .unwrap_or_else(|| row.id.clone());
                let profile = row
                    .agent()
                    .map(|a| format!("@{}", a.profile.id))
                    .unwrap_or_else(|| "Agent".to_owned());
                let started_at = row
                    .terminal
                    .as_ref()
                    .and_then(|t| crate::components::activity::agent_started_at(s, t));
                running.push(RunningItem {
                    terminal_id,
                    card_id: Some(row.card.id.clone()),
                    title: format!("#{} {}", row.card.number, row.card.title),
                    subtitle: format!("{} · {profile}", row.project_name),
                    started_at,
                    is_blocked: row
                        .terminal
                        .as_ref()
                        .is_some_and(|t| t.herdr_status() == HerdrStatus::Blocked),
                });
            }
        }

        let mut terminal_alerts = Vec::new();
        for e in entries.iter() {
            if e.agent.is_some() || !e.listed() {
                continue;
            }
            if e.needs_attention() || e.herdr_status().is_attention() {
                terminal_alerts.push(TerminalAlertItem {
                    id: e.saved.id.clone(),
                    title: e.title(),
                    detail: e.project_name.clone(),
                });
            } else if e.herdr_status().is_working() {
                running.push(RunningItem {
                    terminal_id: e.saved.id.clone(),
                    card_id: e.card.as_ref().map(|c| c.id.clone()),
                    title: e.title(),
                    subtitle: if e.project_name.is_empty() {
                        "Shell".to_owned()
                    } else {
                        e.project_name.clone()
                    },
                    started_at: None,
                    is_blocked: false,
                });
            }
        }

        let mut updates = Vec::new();
        for card in &s.cards {
            if card.removed_at.is_some() || seen_cards.contains(&card.id) {
                continue;
            }
            let unread = counts.get(&card.id).map(|c| c.unread).unwrap_or(0);
            if unread == 0 {
                continue;
            }
            let project_name = s
                .projects
                .iter()
                .find(|p| p.id == card.project_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Project".to_owned());
            let last_seq = s
                .card_summaries
                .get(&card.id)
                .map(|sm| sm.last_event_seq)
                .unwrap_or(0);
            updates.push(UpdateItem {
                card_id: card.id.clone(),
                card_number: card.number,
                card_title: card.title.clone(),
                project_name,
                unread_count: unread,
                last_seq,
            });
        }

        InboxModel {
            action_items,
            terminal_alerts,
            updates,
            running,
        }
    })
}

fn approve_action(ui: Ui, item: &ActionItem) {
    trigger_haptic(Haptic::Selection);
    if let Some((run_id, visit, is_deliver)) = &item.workflow_run {
        if !crate::backend::is_connected() {
            let cid = item.card_id.clone();
            ui.snapshot.update(|s| {
                if let Some(c) = s.cards.iter_mut().find(|c| c.id == cid) {
                    c.human_gate = None;
                    c.attention = None;
                }
            });
            ui.notify("Approved");
            return;
        }
        if *is_deliver {
            dispatch(
                ui,
                "workflow.deliver",
                json!({"card_id": item.card_id, "revision": item.card_revision}),
            );
        } else {
            dispatch(
                ui,
                "workflow.decide",
                json!({
                    "run_id": run_id,
                    "visit": visit,
                    "revision": item.card_revision,
                    "decision": "approve",
                }),
            );
        }
        return;
    }

    if !crate::backend::is_connected() {
        let cid = item.card_id.clone();
        ui.snapshot.update(|s| {
            if let Some(c) = s.cards.iter_mut().find(|c| c.id == cid) {
                c.human_gate = None;
                c.attention = None;
            }
        });
        ui.notify("Approved");
        return;
    }

    // Fallback when a human gate has no attached approval run: clear or advance card stage.
    if let Some(next_stage) = ui.snapshot.with_untracked(|s| {
        let card = s.cards.iter().find(|c| c.id == item.card_id)?;
        let proj = s.projects.iter().find(|p| p.id == card.project_id)?;
        let idx = proj
            .workflow
            .stages
            .iter()
            .position(|st| st.id == card.stage)?;
        proj.workflow.stages.get(idx + 1).map(|st| st.id.clone())
    }) {
        move_card(ui, &item.card_id, &next_stage);
    }
}

pub(crate) fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let scroll_ref = element_ref();
    let model = computed(move || build_inbox_model(ui));

    let body = pull_refresh_scroll(
        scroll().child(view().w_full().p_4().gap_4().child(dynamic(move || {
            let m = model.get();
            if m.action_items.is_empty()
                && m.terminal_alerts.is_empty()
                && m.updates.is_empty()
                && m.running.is_empty()
            {
                return panel()
                    .p_5()
                    .gap_2()
                    .items_center()
                    .child(heading("All caught up"))
                    .child(muted(
                        "No approvals, alerts, or running agents need you right now.",
                    ))
                    .into_element();
            }

            let mut col = view().w_full().gap_4();

            if !m.action_items.is_empty() || !m.terminal_alerts.is_empty() {
                let mut sec = view().w_full().gap_2().child(
                    label("NEEDS ATTENTION")
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(MUTED),
                );
                for item in m.action_items {
                    let approve_item = item.clone();
                    let rework_cid = item.card_id.clone();
                    let reply_cid = item.card_id.clone();
                    let open_cid = item.card_id.clone();
                    let open_term = item.terminal_id.clone();
                    let has_workflow_run = item.workflow_run.is_some();
                    let reasons = item.reasons.join(" · ");

                    let card_box = panel().p_3().gap_2().child(
                        press_target()
                            .w_full()
                            .items_start()
                            .gap_1()
                            .cursor_pointer()
                            .child(
                                view()
                                    .w_full()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(muted(item.project_name))
                                    .child(status("Needs review", Tone::Attention)),
                            )
                            .child(
                                label(format!("#{} {}", item.card_number, item.card_title))
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
                            .children((!reasons.is_empty()).then(|| muted(reasons)))
                            .on_press(move |_: &()| {
                                trigger_haptic(Haptic::Selection);
                                state.push_card(open_cid.clone(), CardSegment::Conversation);
                            }),
                    );

                    let mut actions = view().flex_row().flex_wrap().items_center().gap_2().pt_1();
                    actions = actions.child(compact_action(
                        if has_workflow_run {
                            "Approve"
                        } else {
                            "Advance"
                        },
                        Variant::Primary,
                        move |_| approve_action(ui, &approve_item),
                    ));
                    actions =
                        actions.child(compact_action("Reject", Variant::Secondary, move |_| {
                            trigger_haptic(Haptic::Selection);
                            state.open_sheet(super::MobileSheet::Rework(rework_cid.clone()));
                        }));
                    actions = actions.child(compact_action("Reply", Variant::Quiet, move |_| {
                        state.push_card(reply_cid.clone(), CardSegment::Conversation);
                    }));
                    if let Some(tid) = open_term {
                        actions =
                            actions.child(compact_action("Terminal", Variant::Quiet, move |_| {
                                state.push_terminal(tid.clone())
                            }));
                    }
                    sec = sec.child(card_box.child(actions));
                }
                for alert in m.terminal_alerts {
                    let tid = alert.id.clone();
                    sec = sec.child(
                        panel().p_3().gap_2().child(
                            press_target()
                                .w_full()
                                .items_start()
                                .gap_1()
                                .cursor_pointer()
                                .child(
                                    view()
                                        .w_full()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .child(label(alert.title).font_weight(FontWeight::SEMIBOLD))
                                        .child(status("Input needed", Tone::Attention)),
                                )
                                .children((!alert.detail.is_empty()).then(|| muted(alert.detail)))
                                .on_press(move |_: &()| state.push_terminal(tid.clone())),
                        ),
                    );
                }
                col = col.child(sec);
            }

            if !m.updates.is_empty() {
                let mut sec = view().w_full().gap_2().child(
                    view()
                        .w_full()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            label("UPDATES")
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(MUTED),
                        )
                        .child(compact_action("Mark all read", Variant::Quiet, move |_| {
                            crate::overview::mark_all_read(ui, None);
                        })),
                );
                for u in m.updates {
                    let cid = u.card_id.clone();
                    let read_cid = u.card_id.clone();
                    let seq = u.last_seq;
                    sec = sec.child(
                        panel().p_3().child(
                            press_target()
                                .w_full()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .cursor_pointer()
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .gap_0p5()
                                        .child(
                                            label(format!("#{} {}", u.card_number, u.card_title))
                                                .font_weight(FontWeight::MEDIUM)
                                                .truncate(),
                                        )
                                        .child(muted(u.project_name)),
                                )
                                .child(badge(format!("{} new", u.unread_count)))
                                .on_press(move |_: &()| {
                                    crate::overview::mark_card_read(ui, read_cid.clone(), seq);
                                    state.push_card(cid.clone(), CardSegment::Conversation);
                                }),
                        ),
                    );
                }
                col = col.child(sec);
            }

            if !m.running.is_empty() {
                let mut sec = view().w_full().gap_2().child(
                    label("RUNNING")
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(MUTED),
                );
                for run in m.running {
                    let tid = run.terminal_id.clone();
                    let started_at = run.started_at;
                    sec = sec.child(
                        panel().p_3().child(
                            press_target()
                                .w_full()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .cursor_pointer()
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .child(if run.is_blocked {
                                            view()
                                                .w(px(8.))
                                                .h(px(8.))
                                                .rounded_full()
                                                .bg(ATTENTION)
                                                .into_element()
                                        } else {
                                            live_dot(ACCENT).into_element()
                                        })
                                        .child(
                                            view()
                                                .flex_1()
                                                .min_w(px(0.))
                                                .gap_0p5()
                                                .child(
                                                    label(run.title)
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .truncate(),
                                                )
                                                .child(muted(run.subtitle).truncate()),
                                        ),
                                )
                                .child(muted(move || {
                                    if let Some(ts) = started_at {
                                        crate::components::activity::compact_duration(
                                            ui.runtime_clock.get().saturating_sub(ts),
                                        )
                                    } else {
                                        "Open →".to_owned()
                                    }
                                }))
                                .on_press(move |_: &()| state.push_terminal(tid.clone())),
                        ),
                    );
                }
                col = col.child(sec);
            }

            col.into_element()
        }))),
        scroll_ref,
        move || ui.refreshing.get(),
        move || {
            crate::hosts::reconnect_host(ui, None);
            refresh(ui);
        },
    );

    view()
        .size_full()
        .flex_col()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(
            view()
                .w_full()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .child(heading("Needs you")),
        )
        .child(offline_sync_banner(ui, state, None))
        .child(body)
}
