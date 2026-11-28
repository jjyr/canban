//! Pushed mobile Card workspace: horizontally swipeable `Conversation | Terminal | Diff`
//! (`Carousel` synced with top `SegmentedControl`), flat hairline context & timeline,
//! Workflow Retry, Edit Card sheet, `@agent` quick-mention composer, and Rework sheet.
use super::{
    CardSegment, MobileSheet, MobileState,
    chrome::{
        MOBILE_BODY, empty_state, hairline_divider, is_android, is_ios, is_mobile_os, mobile_body,
        mobile_button, mobile_field, mobile_footnote, mobile_inline_button,
        mobile_segmented_control, mobile_subhead, mobile_title, offline_sync_banner,
        section_header, surface_block, trigger_haptic,
    },
    diff,
};
use crate::{
    card::timeline::entries as timeline_entries, components::timeline_item::timeline_item,
    presenter::*,
};
use canban_terminal::TerminalView;
use canban_types::{Card, Event};
use canban_ui_kit::{tokens::*, *};
use nsh_base::{MenuButtonItem, carousel, carousel_content, carousel_item, menu_button, nav_page};
use nsh_core::{
    Haptic,
    element::{desktop::MenuItem, header::HeaderItem},
    prelude::*,
};
use serde_json::json;

#[derive(Clone, PartialEq)]
enum WorkflowRetryKind {
    Deliver,
    Agent(String),
}

#[derive(Clone, PartialEq)]
struct CardHeaderModel {
    id: String,
    number: u64,
    title: String,
    stage: String,
    project_id: String,
    host_id: String,
    host_offline: bool,
    body: String,
    branch: Option<String>,
    revision: u64,
    sync_mark: Option<canban_types::SyncMark>,
    human_reason: Option<String>,
    approval_run: Option<(String, u32, bool)>, // (run_id, visit, is_deliver)
    retry_info: Option<(WorkflowRetryKind, String)>,
    stages: Vec<(String, String)>,
    assignee: Option<String>,
    active: bool,
    profiles: Vec<String>,
}

fn build_card_model(ui: Ui, card_id: &str) -> Option<CardHeaderModel> {
    let profiles: Vec<String> = ui
        .profiles
        .get()
        .into_iter()
        .filter(|p| p.enabled)
        .map(|p| p.id)
        .collect();
    ui.snapshot.with(|s| {
        let card: &Card = s.cards.iter().find(|c| c.id == card_id)?;
        let host_id = s.card_host(&card.id).to_owned();
        let host_offline = s.is_host_offline(&host_id);
        let sync_mark = s.pending_sync.get(&card.id).cloned();
        let human_reason = card
            .human_gate
            .as_ref()
            .map(|g| g.reason.clone())
            .or_else(|| card.attention.clone())
            .filter(|r| !r.trim().is_empty());
        let approval_run = s
            .workflow_runs
            .iter()
            .find(|r| {
                r.active()
                    && r.card_id.as_deref() == Some(card.id.as_str())
                    && matches!(r.state.as_str(), "waiting_approval" | "stale")
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

        // Detect stuck/failed workflow or replaced-core agent eligible for Retry.
        let mut retry_info: Option<(WorkflowRetryKind, String)> = None;
        for run in s
            .workflow_runs
            .iter()
            .filter(|r| r.active() && r.card_id.as_deref() == Some(card.id.as_str()))
        {
            let is_deliver_step = run.step.as_ref().is_some_and(|k| {
                matches!(
                    run.definition.steps.get(k),
                    Some(canban_types::WorkflowStep::Deliver { .. })
                )
            });
            let active_agent = run.terminal_id.as_deref().and_then(|tid| {
                s.terminals.iter().find(|t| t.id == tid).and_then(|t| {
                    let a = t.agent.as_ref()?;
                    let core_replaced = a.connection_state == "replaced";
                    let stopped = core_replaced
                        || a.terminal.is_none()
                        || a.observed.as_ref().is_some_and(|v| {
                            matches!(v["state"].as_str(), Some("finished" | "missing"))
                        });
                    let retryable = stopped
                        && (core_replaced
                            || matches!(
                                a.state.as_str(),
                                "failed" | "blocked" | "needs_report" | "missing" | "unknown"
                            ));
                    let err = a
                        .error
                        .clone()
                        .or_else(|| core_replaced.then(|| "Herdr core restarted".to_owned()));
                    Some((tid.to_owned(), retryable, err))
                })
            });
            let agent_retryable = active_agent.as_ref().is_some_and(|(_, r, _)| *r);
            if is_deliver_step
                && (run.state == "waiting_delivery"
                    || (run.state == "running" && (run.error.is_some() || agent_retryable)))
            {
                let msg = run
                    .error
                    .clone()
                    .or_else(|| active_agent.as_ref().and_then(|(_, _, e)| e.clone()))
                    .unwrap_or_else(|| "Delivery paused".to_owned());
                retry_info = Some((WorkflowRetryKind::Deliver, msg));
                break;
            }
            if !is_deliver_step
                && agent_retryable
                && let Some((tid, _, err)) = active_agent
            {
                let msg = err
                    .or_else(|| run.error.clone())
                    .unwrap_or_else(|| "Agent stopped".to_owned());
                retry_info = Some((WorkflowRetryKind::Agent(tid), msg));
                break;
            }
        }

        let stages = s
            .projects
            .iter()
            .find(|p| p.id == card.project_id)
            .map(|p| {
                p.workflow
                    .stages
                    .iter()
                    .map(|st| (st.id.clone(), st.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Some(CardHeaderModel {
            id: card.id.clone(),
            number: card.number,
            title: card.title.clone(),
            stage: card.stage.clone(),
            project_id: card.project_id.clone(),
            host_id,
            host_offline,
            body: card.body.clone(),
            branch: card.branch.clone().filter(|b| !b.is_empty()),
            revision: card.revision,
            sync_mark,
            human_reason,
            approval_run,
            retry_info,
            stages,
            assignee: card.assignee.clone(),
            active: card.active(),
            profiles,
        })
    })
}

fn card_events(ui: Ui, card_id: &str) -> Vec<Event> {
    ui.card_timelines
        .with(|m| m.get(card_id).map(|st| st.events.clone()))
        .unwrap_or_default()
}

fn deliver_card(ui: Ui, card_id: &str, rev: u64, has_deliver_run: bool) {
    trigger_haptic(Haptic::Selection);
    if !crate::backend::is_connected() {
        ui.notify("Delivered");
        return;
    }
    if has_deliver_run {
        dispatch(
            ui,
            "workflow.deliver",
            json!({"card_id": card_id, "revision": rev}),
        );
    } else if let Some(card) = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned())
    {
        crate::card::quick_merge_start(ui, card);
    }
}

fn conversation_view(
    ui: Ui,
    state: MobileState,
    card_id: String,
    model: Memo<Option<CardHeaderModel>>,
) -> impl IntoElement {
    let user_toggled_details = signal(Option::<bool>::None);
    let draft = signal(String::new());
    let sending = signal(false);
    let show_mentions = signal(false);

    let events_memo = computed({
        let cid = card_id.clone();
        move || {
            let raw = card_events(ui, &cid);
            timeline_entries(&raw)
        }
    });

    let details_open = computed(move || {
        user_toggled_details.get().unwrap_or_else(|| {
            !events_memo.with(|entries| entries.iter().any(|(_, narrative)| *narrative))
                && model.get().is_some_and(|m| !m.body.trim().is_empty())
        })
    });

    let send_cid = card_id.clone();
    let send_comment = move || {
        let text_val = draft.get_untracked().trim().to_owned();
        if text_val.is_empty() || sending.get_untracked() {
            return;
        }
        trigger_haptic(Haptic::Selection);
        show_mentions.set(false);
        if !crate::backend::is_connected() {
            let pid = model
                .get_untracked()
                .map(|m| m.project_id)
                .unwrap_or_default();
            let ev = Event {
                id: format!("ev-{}", canban_types::id()),
                project_id: pid,
                card_id: send_cid.clone(),
                sequence: 999,
                kind: "comment.created".into(),
                actor: "user".into(),
                message: text_val,
                data: json!({}),
                created_at: canban_types::now(),
            };
            ui.card_timelines.update(|m| {
                m.entry(send_cid.clone()).or_default().events.push(ev);
            });
            draft.set(String::new());
            return;
        }
        sending.set(true);
        let cid = send_cid.clone();
        dispatch_complete(
            ui,
            "comment.add",
            json!({"card_id": cid, "body": text_val}),
            move |res| {
                sending.try_set(false);
                if res.is_ok() {
                    draft.try_set(String::new());
                    ui.ensure_card_timeline(cid);
                }
            },
        );
    };

    view()
        .flex_1()
        .flex_col()
        .min_h(px(0.))
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .child(view().w_full().p_4().gap_4().child(dynamic(move || {
                    let Some(m) = model.get() else {
                        return mobile_subhead("Card not found").into_element();
                    };
                    let mut col = view().w_full().gap_4();

                    if let Some((retry_kind, err_msg)) = m.retry_info.clone() {
                        let cid_retry = m.id.clone();
                        let rev_retry = m.revision;
                        let btn_lbl = match &retry_kind {
                            WorkflowRetryKind::Deliver => "Retry auto-merge",
                            WorkflowRetryKind::Agent(_) => "Retry",
                        };
                        col = col.child(
                            surface_block()
                                .bg(DANGER_SURFACE)
                                .flex_row()
                                .items_stretch()
                                .child(view().w(px(3.)).bg(DANGER).flex_shrink_0())
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .p_3p5()
                                        .gap_2()
                                        .child(
                                            text("Workflow paused")
                                                .text_size(px(14.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(DANGER),
                                        )
                                        .child(mobile_body(err_msg))
                                        .child(mobile_inline_button(
                                            btn_lbl,
                                            Variant::Primary,
                                            move |_| {
                                                if !crate::backend::is_connected() {
                                                    ui.notify("Retrying workflow");
                                                    return;
                                                }
                                                match &retry_kind {
                                                    WorkflowRetryKind::Deliver => {
                                                        dispatch(
                                                            ui,
                                                            "workflow.deliver",
                                                            json!({
                                                                "card_id": cid_retry,
                                                                "revision": rev_retry,
                                                            }),
                                                        );
                                                    }
                                                    WorkflowRetryKind::Agent(tid) => {
                                                        dispatch(
                                                            ui,
                                                            "execution.retry",
                                                            json!({ "terminal_id": tid }),
                                                        );
                                                    }
                                                }
                                            },
                                        )),
                                ),
                        );
                    }

                    if let Some(reason) = m.human_reason {
                        let cid_rework = m.id.clone();
                        let cid_deliver = m.id.clone();
                        let run = m.approval_run.clone();
                        let rev = m.revision;
                        let mut bar = view().w_full().flex_row().gap_2();
                        bar = bar.child(
                            mobile_inline_button("Rework", Variant::Secondary, move |_| {
                                state.open_sheet(MobileSheet::Rework(cid_rework.clone()));
                            })
                            .flex_1(),
                        );
                        if let Some((run_id, visit, is_deliver)) = run {
                            bar = bar.child(
                                mobile_inline_button(
                                    if is_deliver { "Deliver" } else { "Approve" },
                                    Variant::Primary,
                                    move |_| {
                                        if !crate::backend::is_connected() {
                                            ui.notify("Approved");
                                            return;
                                        }
                                        if is_deliver {
                                            deliver_card(ui, &cid_deliver, rev, true);
                                        } else {
                                            dispatch(
                                                ui,
                                                "workflow.decide",
                                                json!({
                                                    "run_id": run_id,
                                                    "visit": visit,
                                                    "revision": rev,
                                                    "decision": "approve",
                                                }),
                                            );
                                        }
                                    },
                                )
                                .flex_1(),
                            );
                        }
                        col = col.child(
                            surface_block()
                                .bg(ATTENTION_SURFACE)
                                .flex_row()
                                .items_stretch()
                                .child(view().w(px(3.)).bg(ATTENTION).flex_shrink_0())
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .p_3p5()
                                        .gap_2p5()
                                        .child(
                                            text("Needs review")
                                                .text_size(px(14.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(ATTENTION),
                                        )
                                        .child(mobile_body(reason))
                                        .child(bar),
                                ),
                        );
                    }

                    let sync_chip = m.sync_mark.map(|mark| {
                        let (lbl, tone) = crate::hosts::sync_chip_spec(mark);
                        status(lbl, tone)
                    });
                    let assignee_chip = m
                        .assignee
                        .as_ref()
                        .map(|a| status(format!("@{a}"), Tone::Neutral));
                    let context_body = m.body.clone();
                    let has_body = !context_body.trim().is_empty();
                    let branch_label = m.branch.clone();
                    let stage_label = m
                        .stages
                        .iter()
                        .find(|(sid, _)| sid == &m.stage)
                        .map(|(_, sname)| sname.clone())
                        .unwrap_or_else(|| m.stage.clone());
                    let edit_cid = m.id.clone();

                    let mut ctx = view().w_full().gap_2p5().child(
                        view()
                            .w_full()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                view()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_1p5()
                                    .child(status(stage_label, Tone::Info))
                                    .children(assignee_chip)
                                    .children(sync_chip)
                                    .children(branch_label.map(|b| {
                                        mobile_footnote(b).font_family(MONO_FONT).truncate()
                                    })),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .flex_shrink_0()
                                    .child(
                                        press_target()
                                            .min_h(px(30.))
                                            .px_1p5()
                                            .items_center()
                                            .justify_center()
                                            .active(|s| s.opacity(0.65))
                                            .child(
                                                text("Edit")
                                                    .text_size(px(13.5))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(ACCENT),
                                            )
                                            .on_press(move |_: &()| {
                                                trigger_haptic(Haptic::Selection);
                                                state.open_sheet(MobileSheet::EditCard(
                                                    edit_cid.clone(),
                                                ));
                                            }),
                                    )
                                    .children(has_body.then(|| {
                                        press_target()
                                            .min_h(px(30.))
                                            .px_1p5()
                                            .items_center()
                                            .justify_center()
                                            .active(|s| s.opacity(0.65))
                                            .child(
                                                text(move || {
                                                    if details_open.get() {
                                                        "Hide"
                                                    } else {
                                                        "Details"
                                                    }
                                                })
                                                .text_size(px(13.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(MUTED),
                                            )
                                            .on_press(move |_: &()| {
                                                let next = !details_open.get_untracked();
                                                user_toggled_details.set(Some(next));
                                            })
                                    })),
                            ),
                    );
                    if has_body {
                        ctx = ctx.child(when(
                            move || details_open.get(),
                            move || {
                                view()
                                    .w_full()
                                    .p_3p5()
                                    .rounded(px(10.))
                                    .bg(SURFACE)
                                    .child(read_text(context_body.clone()))
                            },
                        ));
                    }
                    ctx = ctx.child(hairline_divider());
                    col = col.child(ctx);

                    let rows = events_memo.get();
                    if rows.is_empty() {
                        col = col.child(
                            view()
                                .w_full()
                                .py_6()
                                .items_center()
                                .child(mobile_footnote("No replies yet").text_center()),
                        );
                    } else {
                        for (ev, _) in rows {
                            col = col.child(timeline_item(ui, ev));
                        }
                    }

                    col.into_element()
                }))),
        )
        .child(when(
            move || {
                show_mentions.get()
                    && model.with(|m| m.as_ref().is_some_and(|cm| !cm.profiles.is_empty()))
            },
            move || {
                let profiles = model.get().map(|m| m.profiles).unwrap_or_default();
                view()
                    .w_full()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(BORDER)
                    .bg(BACKGROUND)
                    .child(
                        scroll().horizontal().scrollbar(false).w_full().child(
                            view()
                                .px_3p5()
                                .py_2()
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .children(profiles.into_iter().map(|pid| {
                                    let insert = pid.clone();
                                    press_target()
                                        .h(px(30.))
                                        .px_2p5()
                                        .rounded_full()
                                        .bg(SELECTED)
                                        .items_center()
                                        .justify_center()
                                        .active(|s| s.opacity(0.7))
                                        .child(
                                            text(format!("@{pid}"))
                                                .text_size(px(13.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(ACCENT),
                                        )
                                        .on_press(move |_: &()| {
                                            trigger_haptic(Haptic::Selection);
                                            draft.update(|d| {
                                                if !d.is_empty()
                                                    && !d.ends_with(' ')
                                                    && !d.ends_with('\n')
                                                {
                                                    d.push(' ');
                                                }
                                                d.push('@');
                                                d.push_str(&insert);
                                                d.push(' ');
                                            });
                                            show_mentions.set(false);
                                        })
                                })),
                        ),
                    )
            },
        ))
        .child(
            view()
                .w_full()
                .flex_shrink_0()
                .border_t_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .px_3()
                .pt_2p5()
                .pb(move || {
                    if is_mobile_os() {
                        px(keyboard_height().0.max(safe_area().bottom.0).max(10.))
                    } else {
                        px(10.)
                    }
                })
                .child(
                    view()
                        .w_full()
                        .rounded(px(22.))
                        .bg(INSET)
                        .pl_2()
                        .pr_1p5()
                        .py_1p5()
                        .flex_row()
                        .items_end()
                        .gap_1p5()
                        .child(
                            press_target()
                                .w(px(34.))
                                .h(px(34.))
                                .rounded_full()
                                .bg(move || {
                                    if show_mentions.get() {
                                        SELECTED.resolve()
                                    } else {
                                        CLEAR.resolve()
                                    }
                                })
                                .items_center()
                                .justify_center()
                                .flex_shrink_0()
                                .active(|s| s.opacity(0.65))
                                .accessibility_label(Some(SharedString::from("Mention agent")))
                                .child(
                                    text("@")
                                        .text_size(px(16.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(move || {
                                            if show_mentions.get() {
                                                ACCENT.resolve()
                                            } else {
                                                MUTED.resolve()
                                            }
                                        }),
                                )
                                .on_press(move |_: &()| {
                                    trigger_haptic(Haptic::Selection);
                                    show_mentions.update(|v| *v = !*v);
                                }),
                        )
                        .child(
                            nsh_base::input()
                                .value(move || draft.get())
                                .on_change(move |v: &str| draft.set(v.to_owned()))
                                .multiline(true)
                                .placeholder("Reply or @agent…")
                                .flex_1()
                                .min_w(px(0.))
                                .min_h(px(34.))
                                .max_h(px(110.))
                                .py_1p5()
                                .text_size(px(MOBILE_BODY))
                                .line_height(px(21.))
                                .text_color(TEXT),
                        )
                        .child({
                            let send_cb = send_comment.clone();
                            let can_send =
                                move || !sending.get() && !draft.with(|d| d.trim().is_empty());
                            press_target()
                                .w(px(34.))
                                .h(px(34.))
                                .rounded_full()
                                .items_center()
                                .justify_center()
                                .flex_shrink_0()
                                .bg(move || {
                                    if can_send() {
                                        ACCENT_FILL.resolve()
                                    } else {
                                        BORDER.resolve()
                                    }
                                })
                                .active(|s| s.opacity(0.7))
                                .disabled(move || !can_send())
                                .accessibility_label(Some(SharedString::from("Send reply")))
                                .child(
                                    icon_tinted(Icon::ArrowUp, ON_ACCENT)
                                        .w(px(17.))
                                        .h(px(17.))
                                        .opacity(
                                            move || if can_send() { 1.0_f32 } else { 0.45_f32 },
                                        ),
                                )
                                .on_press(move |_: &()| send_cb())
                        }),
                ),
        )
}

fn card_terminal_view(
    ui: Ui,
    state: MobileState,
    card_id: String,
    model: Memo<Option<CardHeaderModel>>,
    is_visible: impl Fn() -> bool + Copy + 'static,
) -> impl IntoElement {
    let entries_memo = computed({
        let cid = card_id.clone();
        move || crate::terminals::card_live_entries_for_ui(ui, &cid)
    });
    let host_offline = computed(move || model.get().is_some_and(|m| m.host_offline));
    let focus_ticket = signal(1_u64);
    let selected_tid = signal(String::new());

    let active_entry = computed(move || {
        let rows = entries_memo.get();
        let chosen = selected_tid.get();
        if !chosen.is_empty()
            && let Some(found) = rows.iter().find(|e| e.saved.id == chosen)
        {
            return Some(found.clone());
        }
        rows.first().cloned()
    });

    let cid_bar = card_id.clone();
    view()
        .flex_1()
        .flex_col()
        .min_h(px(0.))
        .child(when(
            move || !entries_memo.with(Vec::is_empty),
            move || {
                let cid_new_bar = cid_bar.clone();
                view()
                    .w_full()
                    .flex_shrink_0()
                    .h(px(46.))
                    .px_4()
                    .border_b_1()
                    .border_color(BORDER)
                    .bg(BACKGROUND)
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        scroll()
                            .horizontal()
                            .scrollbar(false)
                            .flex_1()
                            .min_w(px(0.))
                            .child(dynamic(move || {
                                let rows = entries_memo.get();
                                let active_id = active_entry
                                    .get()
                                    .map(|e| e.saved.id)
                                    .unwrap_or_default();
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_1p5()
                                    .children(rows.into_iter().map(|entry| {
                                        let eid = entry.saved.id.clone();
                                        let eid_press = eid.clone();
                                        let is_sel = eid == active_id;
                                        press_target()
                                            .h(px(30.))
                                            .px_2p5()
                                            .rounded_full()
                                            .bg(if is_sel { ACCENT_FILL } else { SURFACE })
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                text(entry.title())
                                                    .text_size(px(13.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(if is_sel { ON_ACCENT } else { TEXT }),
                                            )
                                            .on_press(move |_: &()| {
                                                trigger_haptic(Haptic::Selection);
                                                selected_tid.set(eid_press.clone());
                                                focus_ticket.update(|t| *t += 1);
                                            })
                                    }))
                            })),
                    )
                    .child(
                        press_target()
                            .h(px(30.))
                            .px_2p5()
                            .rounded_full()
                            .bg(SURFACE)
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .active(|s| s.opacity(0.65))
                            .child(icon_tinted(Icon::Plus, ACCENT).w(px(13.)).h(px(13.)))
                            .child(
                                text("Shell")
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ACCENT),
                            )
                            .on_press(move |_: &()| {
                                trigger_haptic(Haptic::Selection);
                                if let Some(m) = model.get_untracked() {
                                    crate::terminals::create_in_card(
                                        ui,
                                        m.project_id,
                                        cid_new_bar.clone(),
                                        false,
                                    );
                                }
                            }),
                    )
            },
        ))
        .child(dynamic(move || {
            if !is_visible() {
                return view().flex_1().bg(BACKGROUND).into_element();
            }
            let Some(entry) = active_entry.get() else {
                let cid_new = card_id.clone();
                return view()
                    .flex_1()
                    .justify_center()
                    .child(empty_state(
                        Icon::Terminal,
                        Tone::Neutral,
                        "No active terminal",
                        "",
                        Some(
                            mobile_button("Start Shell", Variant::Primary, move |_| {
                                if let Some(m) = model.get_untracked() {
                                    crate::terminals::create_in_card(
                                        ui,
                                        m.project_id,
                                        cid_new.clone(),
                                        false,
                                    );
                                }
                            })
                            .into_element(),
                        ),
                    ))
                    .into_element();
            };

            let focus_req = move || canban_terminal::FocusRequest {
                ticket: focus_ticket.get(),
                enabled: state.sheet.with(Option::is_none) && is_visible(),
            };
            let offline_msg = move || {
                if host_offline.get() {
                    "Terminal needs a connection".to_owned()
                } else {
                    String::new()
                }
            };

            let term_view = if !crate::backend::is_connected() {
                TerminalView::preview(
                    format!(
                        "\x1b[1;36m{}\x1b[0m · Mobile Terminal\r\n$ cargo nextest run --workspace\r\nrunning 931 tests... ok\r\n> ",
                        entry.title()
                    ),
                    crate::utilities::terminal_theme,
                    move || crate::utilities::terminal_font(ui),
                    focus_req,
                )
            } else {
                let bind = entry.binding();
                let core_id = bind
                    .as_ref()
                    .and_then(|v| v["core_id"].as_str())
                    .unwrap_or("")
                    .to_owned();
                let tty = bind
                    .as_ref()
                    .and_then(|v| v["terminal"].as_str())
                    .unwrap_or("")
                    .to_owned();
                let host_id = entry.host_id.clone();
                let ro = entry.ended()
                    || bind
                        .as_ref()
                        .is_some_and(|v| v["read_only"] == true);
                TerminalView::live(
                    move || {
                        (!core_id.is_empty() && !tty.is_empty()).then(|| {
                            canban_terminal::LiveBinding {
                                session_epoch: crate::terminals::host_connect_epoch(ui, &host_id),
                                host_id: host_id.clone(),
                                config: crate::config_for_host(&host_id),
                                core_id: core_id.clone(),
                                terminal: tty.clone(),
                                transport: crate::backend::terminal_transport(&host_id),
                            }
                        })
                    },
                    crate::utilities::terminal_theme,
                    move || crate::utilities::terminal_font(ui),
                    focus_req,
                )
                .read_only(move || ro || host_offline.get())
            };

            term_view
                .mobile_controls(true)
                .offline_notice(offline_msg)
                .on_fullscreen_toggle(move || {
                    state.terminal_fullscreen.update(|f| *f = !*f);
                })
                .into_element()
        }))
}

pub(crate) fn pushed_page(
    ui: Ui,
    state: MobileState,
    card_id: String,
    segment_sig: Signal<CardSegment>,
) -> impl IntoElement {
    ui.ensure_card_timeline(card_id.clone());

    let cid_model = card_id.clone();
    let model = computed(move || build_card_model(ui, &cid_model));
    let pager_width = signal(390.0_f32);

    let cid_actions = card_id.clone();
    let cid_conv = card_id.clone();
    let cid_term = card_id.clone();
    let cid_diff = card_id.clone();

    let title_fn = move || {
        model
            .get()
            .map(|m| format!("#{} {}", m.number, m.title))
            .unwrap_or_else(|| "Card".to_owned())
    };

    let more_menu = {
        let cid_m = cid_actions.clone();
        move || {
            let Some(m) = model.get_untracked() else {
                return Vec::new();
            };
            let cid_edit = cid_m.clone();
            let cid_rework = cid_m.clone();
            let cid_deliv = cid_m.clone();
            let rev = m.revision;
            let has_run = m.approval_run.is_some();

            let stage_items: Vec<MenuItem> = m
                .stages
                .iter()
                .map(|(sid, sname)| {
                    let cid_move = cid_m.clone();
                    let sid_target = sid.clone();
                    MenuItem::new(sname.clone())
                        .checked(sid == &m.stage)
                        .on_select(move || {
                            trigger_haptic(Haptic::Selection);
                            move_card(ui, &cid_move, &sid_target);
                            state.board_stage.set(sid_target.clone());
                        })
                })
                .collect();

            let mut items =
                vec![
                    MenuItem::new("Edit Card")
                        .symbol("pencil")
                        .on_select(move || {
                            trigger_haptic(Haptic::Selection);
                            state.open_sheet(MobileSheet::EditCard(cid_edit.clone()));
                        }),
                ];
            if !stage_items.is_empty() {
                items.push(MenuItem::submenu("Move to…", stage_items).symbol("arrow.right.circle"));
            }
            if m.active && !m.profiles.is_empty() {
                let mut assign_items: Vec<MenuItem> = m
                    .profiles
                    .iter()
                    .map(|pid| {
                        let cid_assign = cid_m.clone();
                        let assign_pid = pid.clone();
                        MenuItem::new(format!("@{pid}"))
                            .checked(m.assignee.as_deref() == Some(pid.as_str()))
                            .on_select(move || {
                                trigger_haptic(Haptic::Selection);
                                assign_card(ui, &cid_assign, Some(&assign_pid));
                            })
                    })
                    .collect();
                if m.assignee.is_some() {
                    let cid_unassign = cid_m.clone();
                    assign_items.push(MenuItem::separator());
                    assign_items.push(MenuItem::new("Unassign").on_select(move || {
                        trigger_haptic(Haptic::Selection);
                        assign_card(ui, &cid_unassign, None);
                    }));
                }
                items.push(
                    MenuItem::submenu("Assign Agent", assign_items).symbol("person.badge.key"),
                );
            }
            items.push(
                MenuItem::new("Return for rework")
                    .symbol("arrow.uturn.backward")
                    .on_select(move || {
                        state.open_sheet(MobileSheet::Rework(cid_rework.clone()));
                    }),
            );
            items.push(
                MenuItem::new("Deliver…")
                    .symbol("paperplane")
                    .on_select(move || {
                        deliver_card(ui, &cid_deliv, rev, has_run);
                    }),
            );
            if let Some(br) = m.branch {
                items.push(MenuItem::separator());
                items.push(MenuItem::new("Copy branch").symbol("doc.on.doc").on_select(
                    move || {
                        copy_text(ui, br.clone());
                    },
                ));
            }
            items
        }
    };

    let android_menu = {
        let cid_a = cid_actions.clone();
        move || {
            let Some(m) = model.get() else {
                return view().into_element();
            };
            let mut items = vec![MenuButtonItem::new("edit", "Edit Card")];
            if !m.stages.is_empty() {
                let stage_items = m.stages.iter().map(|(sid, sname)| {
                    MenuButtonItem::new(format!("stage:{sid}"), sname.clone())
                        .checked(sid == &m.stage)
                });
                items.push(MenuButtonItem::submenu("Move to…", stage_items));
            }
            if m.active && !m.profiles.is_empty() {
                let mut assign_items: Vec<MenuButtonItem> = m
                    .profiles
                    .iter()
                    .map(|pid| {
                        MenuButtonItem::new(format!("assign:{pid}"), format!("@{pid}"))
                            .checked(m.assignee.as_deref() == Some(pid.as_str()))
                    })
                    .collect();
                if m.assignee.is_some() {
                    assign_items.push(MenuButtonItem::new("unassign", "Unassign"));
                }
                items.push(MenuButtonItem::submenu("Assign Agent", assign_items));
            }
            items.push(MenuButtonItem::new("rework", "Return for rework"));
            items.push(MenuButtonItem::new("deliver", "Deliver…"));
            if m.branch.is_some() {
                items.push(MenuButtonItem::new("copy-branch", "Copy branch"));
            }

            let cid_sel = cid_a.clone();
            let rev = m.revision;
            let has_run = m.approval_run.is_some();
            let branch = m.branch.clone();
            menu_button()
                .title("Card actions")
                .accessibility_label(Some(SharedString::from("Card actions")))
                .items(items)
                .on_select(move |choice: &SharedString| {
                    trigger_haptic(Haptic::Selection);
                    let raw = choice.as_ref();
                    if let Some(sid) = raw.strip_prefix("stage:") {
                        move_card(ui, &cid_sel, sid);
                        state.board_stage.set(sid.to_owned());
                    } else if let Some(pid) = raw.strip_prefix("assign:") {
                        assign_card(ui, &cid_sel, Some(pid));
                    } else {
                        match raw {
                            "edit" => state.open_sheet(MobileSheet::EditCard(cid_sel.clone())),
                            "unassign" => assign_card(ui, &cid_sel, None),
                            "rework" => state.open_sheet(MobileSheet::Rework(cid_sel.clone())),
                            "deliver" => deliver_card(ui, &cid_sel, rev, has_run),
                            "copy-branch" => {
                                if let Some(br) = &branch {
                                    copy_text(ui, br.clone());
                                }
                            }
                            _ => {}
                        }
                    }
                })
                .child(
                    view()
                        .w(px(44.))
                        .h(px(44.))
                        .items_center()
                        .justify_center()
                        .child(icon_tinted(Icon::More, ACCENT).w(px(21.)).h(px(21.))),
                )
                .into_element()
        }
    };

    let move_cid_gallery = cid_actions.clone();
    let edit_cid_gallery = cid_actions.clone();
    let desktop_header = view()
        .w_full()
        .h(px(52.))
        .px_3()
        .border_b_1()
        .border_color(BORDER)
        .bg(BACKGROUND)
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            press_target()
                .min_h(px(40.))
                .px_2()
                .flex_row()
                .items_center()
                .gap_1()
                .active(|s| s.opacity(0.65))
                .child(icon_tinted(Icon::Back, ACCENT).w(px(18.)).h(px(18.)))
                .child(
                    text("Back")
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ACCENT),
                )
                .on_press(move |_: &()| state.pop()),
        )
        .child(mobile_title(title_fn).truncate().flex_1().min_w(px(0.)))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(
                    press_target()
                        .min_h(px(34.))
                        .px_2p5()
                        .rounded(px(8.))
                        .bg(SURFACE)
                        .items_center()
                        .justify_center()
                        .child(
                            text("Edit")
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT),
                        )
                        .on_press(move |_: &()| {
                            state.open_sheet(MobileSheet::EditCard(edit_cid_gallery.clone()));
                        }),
                )
                .child(
                    press_target()
                        .w(px(34.))
                        .h(px(34.))
                        .rounded(px(8.))
                        .items_center()
                        .justify_center()
                        .child(icon_tinted(Icon::More, MUTED).w(px(18.)).h(px(18.)))
                        .on_press(move |_: &()| {
                            state.open_sheet(MobileSheet::MoveCard(move_cid_gallery.clone()));
                        }),
                ),
        );

    let host_scope = computed(move || model.get().map(|m| m.host_id).filter(|h| !h.is_empty()));

    let mut page = nav_page()
        .key(format!("mobile-card:{card_id}"))
        .title(title_fn)
        .header_large_title(false)
        .header_shadow_visible(false)
        .header_style_with(|s| s.text_color(TEXT).tint_color(ACCENT))
        .hides_bottom_bar_when_pushed(true)
        .header_visible(move || !state.terminal_fullscreen.get())
        .on_dismiss(move |_: &()| {
            state.terminal_fullscreen.set(false);
            state.pop();
        });

    if is_ios() {
        page = page.header_right_items(move || {
            vec![
                HeaderItem::symbol("ellipsis.circle")
                    .key("card-more")
                    .label("Card actions")
                    .menu(more_menu()),
            ]
        });
    } else if is_android() {
        page = page.header_right(dynamic(android_menu));
    } else {
        page = page.header(desktop_header);
    }

    let conv_slide = carousel_item()
        .accessibility_label(Some(SharedString::from("Conversation")))
        .w(move || px(pager_width.get()))
        .h_full()
        .flex_shrink_0()
        .child(conversation_view(ui, state, cid_conv, model));

    let term_slide = carousel_item()
        .accessibility_label(Some(SharedString::from("Terminal")))
        .w(move || px(pager_width.get()))
        .h_full()
        .flex_shrink_0()
        .child(card_terminal_view(ui, state, cid_term, model, move || {
            segment_sig.get() == CardSegment::Terminal
        }));

    let diff_slide = carousel_item()
        .accessibility_label(Some(SharedString::from("Diff")))
        .w(move || px(pager_width.get()))
        .h_full()
        .flex_shrink_0()
        .child(diff::segment_view(ui, state, cid_diff));

    page.child(
        view()
            .size_full()
            .flex_col()
            .min_h(px(0.))
            .bg(BACKGROUND)
            .child(when(
                move || !state.terminal_fullscreen.get(),
                move || {
                    view()
                        .w_full()
                        .flex_shrink_0()
                        .px_4()
                        .py_2()
                        .bg(BACKGROUND)
                        .child(mobile_segmented_control(
                            &["Conversation", "Terminal", "Diff"],
                            move || segment_sig.get().index(),
                            move |idx| {
                                if let Some(next) = CardSegment::from_index(idx) {
                                    segment_sig.set(next);
                                }
                            },
                        ))
                },
            ))
            .child(dynamic(move || {
                view()
                    .w_full()
                    .px_4()
                    .child(offline_sync_banner(ui, state, host_scope.get()))
            }))
            .child(
                view()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.))
                    .on_layout(move |bounds: &Bounds<Points>| {
                        if bounds.size.width.0 > 50.0 {
                            pager_width.set(bounds.size.width.0);
                        }
                    })
                    .child(
                        carousel()
                            .size_full()
                            .index(move || Some(segment_sig.get().index() as u32))
                            .on_change(move |idx: &u32| {
                                if let Some(next) = CardSegment::from_index(*idx as usize)
                                    && segment_sig.get_untracked() != next
                                {
                                    trigger_haptic(Haptic::Selection);
                                    segment_sig.set(next);
                                }
                            })
                            .child(
                                carousel_content()
                                    .size_full()
                                    .flex_row()
                                    .child(conv_slide)
                                    .child(term_slide)
                                    .child(diff_slide),
                            ),
                    ),
            ),
    )
}

pub(crate) fn edit_card_sheet_body(
    ui: Ui,
    state: MobileState,
    card_id: String,
) -> impl IntoElement {
    let initial = untracked(|| {
        ui.snapshot.with_untracked(|s| {
            s.cards
                .iter()
                .find(|c| c.id == card_id)
                .map(|c| (c.title.clone(), c.body.clone(), c.revision))
        })
    });
    let Some((init_title, init_body, init_rev)) = initial else {
        return view().child(mobile_subhead("Card unavailable"));
    };

    let title = signal(init_title);
    let body = signal(init_body);
    let busy = signal(false);

    view()
        .w_full()
        .gap_4()
        .child(
            view()
                .w_full()
                .gap_1p5()
                .child(section_header("TITLE", None))
                .child(mobile_field(title, "Card title")),
        )
        .child(
            view()
                .w_full()
                .gap_1p5()
                .child(section_header("DESCRIPTION", None))
                .child(
                    nsh_base::input()
                        .value(move || body.get())
                        .on_change(move |v: &str| body.set(v.to_owned()))
                        .multiline(true)
                        .placeholder("Details…")
                        .min_h(px(140.))
                        .px_3p5()
                        .py_2p5()
                        .rounded(px(12.))
                        .bg(INSET)
                        .border_1()
                        .border_color(BORDER)
                        .text_size(px(16.))
                        .text_color(TEXT)
                        .focus(|s| s.border_color(ACCENT)),
                ),
        )
        .child(
            mobile_button(
                move || if busy.get() { "Saving…" } else { "Save" },
                Variant::Primary,
                move |_| {
                    let t = title.get_untracked().trim().to_owned();
                    if t.is_empty() || busy.get_untracked() {
                        return;
                    }
                    let b = body.get_untracked();
                    if !crate::backend::is_connected() {
                        ui.snapshot.update(|s| {
                            if let Some(c) = s.cards.iter_mut().find(|c| c.id == card_id) {
                                c.title = t;
                                c.body = b;
                                c.revision += 1;
                            }
                        });
                        state.close_sheet();
                        ui.notify("Saved");
                        return;
                    }
                    let rev = ui.snapshot.with_untracked(|s| {
                        s.cards
                            .iter()
                            .find(|c| c.id == card_id)
                            .map(|c| c.revision)
                            .unwrap_or(init_rev)
                    });
                    busy.set(true);
                    dispatch_complete(
                        ui,
                        "card.update",
                        json!({
                            "card_id": card_id,
                            "revision": rev,
                            "title": t,
                            "body": b,
                        }),
                        move |res| {
                            busy.try_set(false);
                            if res.is_ok() {
                                state.close_sheet();
                                ui.notify("Saved");
                            }
                        },
                    );
                },
            )
            .disabled(move || busy.get() || title.get().trim().is_empty()),
        )
}

pub(crate) fn rework_sheet_body(ui: Ui, state: MobileState, card_id: String) -> impl IntoElement {
    let feedback = signal(String::new());
    let busy = signal(false);
    let cid_lookup = card_id.clone();

    let target = computed(move || {
        ui.snapshot.with(|s| {
            let card = s.cards.iter().find(|c| c.id == cid_lookup)?;
            let run = s
                .workflow_runs
                .iter()
                .find(|r| {
                    r.active()
                        && r.card_id.as_deref() == Some(cid_lookup.as_str())
                        && matches!(r.state.as_str(), "waiting_approval" | "stale")
                })
                .map(|r| (r.id.clone(), r.visit));
            Some((card.number, card.title.clone(), card.revision, run))
        })
    });

    dynamic(move || {
        let Some((num, title, rev, run_opt)) = target.get() else {
            return mobile_subhead("Card unavailable").into_element();
        };
        let cid_submit = card_id.clone();
        view()
            .w_full()
            .gap_4()
            .child(mobile_title(format!("#{num} {title}")))
            .child(
                nsh_base::input()
                    .value(move || feedback.get())
                    .on_change(move |v: &str| feedback.set(v.to_owned()))
                    .multiline(true)
                    .placeholder("Feedback…")
                    .min_h(px(120.))
                    .px_3p5()
                    .py_2p5()
                    .rounded(px(12.))
                    .bg(INSET)
                    .border_1()
                    .border_color(BORDER)
                    .text_size(px(MOBILE_BODY))
                    .text_color(TEXT)
                    .focus(|s| s.border_color(ACCENT)),
            )
            .child(
                mobile_button(
                    move || if busy.get() { "Submitting…" } else { "Return for Rework" },
                    Variant::Primary,
                    move |_| {
                        if busy.get_untracked() {
                            return;
                        }
                        let note = feedback.get_untracked();
                        if !crate::backend::is_connected() {
                            ui.notify("Returned for rework");
                            state.close_sheet();
                            return;
                        }
                        if let Some((run_id, visit)) = &run_opt {
                            busy.set(true);
                            dispatch_complete(
                                ui,
                                "workflow.decide",
                                json!({
                                    "run_id": run_id,
                                    "visit": visit,
                                    "revision": rev,
                                    "decision": "rework",
                                    "feedback": note,
                                }),
                                move |_| {
                                    busy.try_set(false);
                                    state.close_sheet();
                                },
                            );
                        } else if !note.trim().is_empty() {
                            busy.set(true);
                            dispatch_complete(
                                ui,
                                "comment.add",
                                json!({"card_id": cid_submit, "body": format!("Return for rework:\n\n{note}")}),
                                move |_| {
                                    busy.try_set(false);
                                    state.close_sheet();
                                },
                            );
                        } else {
                            state.close_sheet();
                        }
                    },
                )
                .disabled(move || busy.get()),
            )
            .into_element()
    })
}
