//! `Needs you` mobile inbox tab: actionable approvals/attentions, unread updates, and running agents.
use super::{
    CardSegment, MobileSheet, MobileState,
    chrome::{
        MOBILE_CARD_RADIUS, disclosure_chevron, empty_state, grouped_card, grouped_card_with_inset,
        icon_badge, is_mobile_os, mobile_footnote, mobile_inline_button, mobile_subhead,
        mobile_title, offline_sync_banner, section_header, tab_scroll_bottom_pad, trigger_haptic,
    },
};
use crate::presenter::*;
use canban_types::HerdrStatus;
use canban_ui_kit::{pull_refresh_scroll, tokens::*, *};
use nsh_base::{MenuButtonItem, context_menu};
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

fn render_action_card(ui: Ui, state: MobileState, item: ActionItem) -> AnyElement {
    let approve_item = item.clone();
    let rework_cid = item.card_id.clone();
    let open_cid = item.card_id.clone();
    let diff_cid = item.card_id.clone();
    let term_for_menu = item.terminal_id.clone();
    let term_for_btn = item.terminal_id.clone();
    let rework_menu_cid = item.card_id.clone();
    let is_deliver = item
        .workflow_run
        .as_ref()
        .is_some_and(|(_, _, deliver)| *deliver);
    let has_workflow_run = item.workflow_run.is_some();
    let primary_lbl = if is_deliver {
        "Deliver"
    } else if has_workflow_run {
        "Approve"
    } else {
        "Advance"
    };
    let reasons = item.reasons.join(" · ");

    let tap_body = press_target()
        .w_full()
        .items_start()
        .gap_1p5()
        .cursor_pointer()
        .active(|s| s.opacity(0.75))
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    mobile_footnote(format!("{} · #{}", item.project_name, item.card_number))
                        .tabular_nums()
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                )
                .child(status("Needs review", Tone::Attention)),
        )
        .child(mobile_title(item.card_title))
        .children((!reasons.is_empty()).then(|| mobile_subhead(reasons)))
        .on_press(move |_: &()| {
            trigger_haptic(Haptic::Selection);
            state.push_card(open_cid.clone(), CardSegment::Conversation);
        });

    let mut action_row = view()
        .w_full()
        .flex_row()
        .items_center()
        .gap_2()
        .pt_1()
        .child(
            mobile_inline_button("Rework", Variant::Secondary, move |_| {
                state.open_sheet(MobileSheet::Rework(rework_cid.clone()));
            })
            .flex_1(),
        )
        .child(
            mobile_inline_button(primary_lbl, Variant::Primary, move |_| {
                approve_action(ui, &approve_item);
            })
            .flex_1(),
        );

    if let Some(tid) = term_for_btn {
        action_row = action_row.child(
            press_target()
                .w(px(42.))
                .h(px(42.))
                .rounded(px(11.))
                .bg(INSET)
                .border_1()
                .border_color(BORDER)
                .items_center()
                .justify_center()
                .active(|s| s.opacity(0.75))
                .accessibility_label(Some(SharedString::from("Open terminal")))
                .child(icon_tinted(Icon::Terminal, ACCENT).w(px(18.)).h(px(18.)))
                .on_press(move |_: &()| {
                    trigger_haptic(Haptic::Selection);
                    state.push_terminal(tid.clone());
                }),
        );
    }

    let card = view()
        .w_full()
        .p_4()
        .gap_3()
        .rounded(px(MOBILE_CARD_RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .child(tap_body)
        .child(action_row);

    if !is_mobile_os() {
        return card.into_element();
    }

    let mut menu_items = vec![MenuButtonItem::new("open", "Open Conversation").icon("bubble.left")];
    if term_for_menu.is_some() {
        menu_items.push(MenuButtonItem::new("terminal", "Open Terminal").icon("terminal"));
    }
    menu_items.push(MenuButtonItem::new("diff", "View Diff").icon("doc.text.magnifyingglass"));
    menu_items
        .push(MenuButtonItem::new("rework", "Return for Rework").icon("arrow.uturn.backward"));

    let open_cid_menu = item.card_id;
    context_menu()
        .w_full()
        .items(menu_items)
        .trigger(card)
        .on_select(move |choice: &SharedString| {
            trigger_haptic(Haptic::Selection);
            match choice.as_ref() {
                "open" => state.push_card(open_cid_menu.clone(), CardSegment::Conversation),
                "diff" => state.push_card(diff_cid.clone(), CardSegment::Diff),
                "terminal" => {
                    if let Some(tid) = &term_for_menu {
                        state.push_terminal(tid.clone());
                    }
                }
                "rework" => state.open_sheet(MobileSheet::Rework(rework_menu_cid.clone())),
                _ => {}
            }
        })
        .into_element()
}

pub(crate) fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let scroll_ref = element_ref();
    let model = computed(move || build_inbox_model(ui));

    let body = pull_refresh_scroll(
        scroll().w_full().bg(BACKGROUND).child(
            view()
                .w_full()
                .px_4()
                .pt_2()
                .pb(px(tab_scroll_bottom_pad()))
                .gap_5()
                .child(offline_sync_banner(ui, state, None))
                .child(dynamic(move || {
                    let m = model.get();
                    if m.action_items.is_empty()
                        && m.terminal_alerts.is_empty()
                        && m.updates.is_empty()
                        && m.running.is_empty()
                    {
                        return empty_state(
                            Icon::Check,
                            Tone::Success,
                            "All caught up",
                            "No approvals, alerts, or running agents need you right now.",
                            None,
                        )
                        .into_element();
                    }

                    let mut col = view().w_full().gap_6();

                    if !m.action_items.is_empty() || !m.terminal_alerts.is_empty() {
                        let mut sec = view()
                            .w_full()
                            .gap_2p5()
                            .child(section_header("NEEDS ATTENTION", None));
                        for item in m.action_items {
                            sec = sec.child(render_action_card(ui, state, item));
                        }
                        for alert in m.terminal_alerts {
                            let tid = alert.id.clone();
                            let tid_btn = alert.id.clone();
                            sec = sec.child(
                                view()
                                    .w_full()
                                    .p_4()
                                    .gap_3()
                                    .rounded(px(MOBILE_CARD_RADIUS))
                                    .border_1()
                                    .border_color(BORDER)
                                    .bg(SURFACE)
                                    .child(
                                        press_target()
                                            .w_full()
                                            .gap_1()
                                            .active(|s| s.opacity(0.75))
                                            .child(
                                                view()
                                                    .w_full()
                                                    .flex_row()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap_2()
                                                    .child(
                                                        mobile_title(alert.title)
                                                            .flex_1()
                                                            .min_w(px(0.)),
                                                    )
                                                    .child(status("Input needed", Tone::Attention)),
                                            )
                                            .children(
                                                (!alert.detail.is_empty())
                                                    .then(|| mobile_subhead(alert.detail)),
                                            )
                                            .on_press(move |_: &()| {
                                                trigger_haptic(Haptic::Selection);
                                                state.push_terminal(tid.clone());
                                            }),
                                    )
                                    .child(mobile_inline_button(
                                        "Open Terminal",
                                        Variant::Secondary,
                                        move |_| state.push_terminal(tid_btn.clone()),
                                    )),
                            );
                        }
                        col = col.child(sec);
                    }

                    if !m.updates.is_empty() {
                        let mark_all = press_target()
                            .min_h(px(32.))
                            .px_1()
                            .items_center()
                            .justify_center()
                            .active(|s| s.opacity(0.65))
                            .child(
                                text("Mark all read")
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ACCENT),
                            )
                            .on_press(move |_: &()| {
                                trigger_haptic(Haptic::Selection);
                                crate::overview::mark_all_read(ui, None);
                            })
                            .into_element();

                        let rows: Vec<AnyElement> = m
                            .updates
                            .into_iter()
                            .map(|u| {
                                let cid = u.card_id.clone();
                                let read_cid = u.card_id.clone();
                                let seq = u.last_seq;
                                press_target()
                                    .w_full()
                                    .min_h(px(58.))
                                    .px_4()
                                    .py_2p5()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .active(|s| s.opacity(0.75))
                                    .child(
                                        view()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .gap_0p5()
                                            .child(
                                                text(format!("#{} {}", u.card_number, u.card_title))
                                                    .text_size(px(16.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(TEXT)
                                                    .truncate(),
                                            )
                                            .child(mobile_footnote(u.project_name).truncate()),
                                    )
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .flex_shrink_0()
                                            .child(
                                                view()
                                                    .min_w(px(24.))
                                                    .h(px(24.))
                                                    .px_2()
                                                    .rounded_full()
                                                    .bg(SELECTED)
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        text(u.unread_count.to_string())
                                                            .text_size(px(12.5))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(ACCENT)
                                                            .tabular_nums(),
                                                    ),
                                            )
                                            .child(disclosure_chevron()),
                                    )
                                    .on_press(move |_: &()| {
                                        trigger_haptic(Haptic::Selection);
                                        crate::overview::mark_card_read(ui, read_cid.clone(), seq);
                                        state.push_card(cid.clone(), CardSegment::Conversation);
                                    })
                                    .into_element()
                            })
                            .collect();

                        col = col.child(
                            view()
                                .w_full()
                                .child(section_header("UPDATES", Some(mark_all)))
                                .child(grouped_card(rows)),
                        );
                    }

                    if !m.running.is_empty() {
                        let rows: Vec<AnyElement> = m
                            .running
                            .into_iter()
                            .map(|run| {
                                let tid = run.terminal_id.clone();
                                let started_at = run.started_at;
                                let (ic, fg, bg) = if run.is_blocked {
                                    (Icon::TriangleAlert, ATTENTION, ATTENTION_SURFACE)
                                } else {
                                    (Icon::Agent, ACCENT, SELECTED)
                                };
                                press_target()
                                    .w_full()
                                    .min_h(px(60.))
                                    .px_4()
                                    .py_2p5()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .active(|s| s.opacity(0.75))
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_3()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .child(icon_badge(ic, fg, bg))
                                            .child(
                                                view()
                                                    .flex_1()
                                                    .min_w(px(0.))
                                                    .gap_0p5()
                                                    .child(
                                                        text(run.title)
                                                            .text_size(px(16.))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(TEXT)
                                                            .truncate(),
                                                    )
                                                    .child(mobile_footnote(run.subtitle).truncate()),
                                            ),
                                    )
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .flex_shrink_0()
                                            .child(if let Some(ts) = started_at {
                                                view()
                                                    .flex_row()
                                                    .items_center()
                                                    .gap_1p5()
                                                    .child(live_dot(if run.is_blocked {
                                                        ATTENTION
                                                    } else {
                                                        ACCENT
                                                    }))
                                                    .child(
                                                        text(move || {
                                                            crate::components::activity::compact_duration(
                                                                ui.runtime_clock
                                                                    .get()
                                                                    .saturating_sub(ts),
                                                            )
                                                        })
                                                        .text_size(px(13.))
                                                        .text_color(MUTED)
                                                        .tabular_nums(),
                                                    )
                                                    .into_element()
                                            } else {
                                                live_dot(if run.is_blocked {
                                                    ATTENTION
                                                } else {
                                                    ACCENT
                                                })
                                                .into_element()
                                            })
                                            .child(disclosure_chevron()),
                                    )
                                    .on_press(move |_: &()| {
                                        trigger_haptic(Haptic::Selection);
                                        state.push_terminal(tid.clone());
                                    })
                                    .into_element()
                            })
                            .collect();

                        col = col.child(
                            view()
                                .w_full()
                                .child(section_header("RUNNING", None))
                                .child(grouped_card_with_inset(rows, 60.0)),
                        );
                    }

                    col.into_element()
                })),
        ),
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
        .child(body)
}
