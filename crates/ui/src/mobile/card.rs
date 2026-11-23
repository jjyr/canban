//! Pushed mobile Card workspace: `Conversation | Terminal | Diff`, collapsible context,
//! timeline, keyboard-lifted composer, Card terminal switcher, and `Return for rework` sheet.
use super::{
    CardSegment, MobileSheet, MobileState,
    chrome::{is_ios, mobile_segmented_control, offline_sync_banner, trigger_haptic},
    diff,
};
use crate::{
    card::timeline::entries as timeline_entries, components::timeline_item::timeline_item,
    presenter::*,
};
use canban_terminal::TerminalView;
use canban_types::{Card, Event};
use canban_ui_kit::{tokens::*, *};
use nsh_base::nav_page;
use nsh_core::{
    Haptic,
    element::{desktop::MenuItem, header::HeaderItem},
    platform::Os,
    prelude::*,
};
use serde_json::json;

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
}

fn build_card_model(ui: Ui, card_id: &str) -> Option<CardHeaderModel> {
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
        ui.notify("Gallery preview · Deliver Card");
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
    let details_open = signal(false);
    let draft = signal(String::new());
    let sending = signal(false);

    let events_memo = computed({
        let cid = card_id.clone();
        move || {
            let raw = card_events(ui, &cid);
            timeline_entries(&raw)
        }
    });

    let send_cid = card_id.clone();
    let send_comment = move || {
        let text = draft.get_untracked().trim().to_owned();
        if text.is_empty() || sending.get_untracked() {
            return;
        }
        trigger_haptic(Haptic::Selection);
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
                message: text,
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
            json!({"card_id": cid, "body": text}),
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
                .child(view().w_full().p_3().gap_3().child(dynamic(move || {
                    let Some(m) = model.get() else {
                        return muted("Card not found").into_element();
                    };
                    let mut col = view().w_full().gap_3();

                    if let Some(reason) = m.human_reason {
                        let cid_rework = m.id.clone();
                        let cid_deliver = m.id.clone();
                        let run = m.approval_run.clone();
                        let rev = m.revision;
                        let gate = panel()
                            .p_3()
                            .gap_2()
                            .border_color(ATTENTION)
                            .child(
                                label("Needs review")
                                    .text_color(ATTENTION)
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
                            .child(muted(reason));
                        let mut bar = view().flex_row().flex_wrap().gap_2();
                        if let Some((run_id, visit, is_deliver)) = run {
                            bar = bar.child(compact_action(
                                if is_deliver { "Deliver" } else { "Approve" },
                                Variant::Primary,
                                move |_| {
                                    if !crate::backend::is_connected() {
                                        ui.notify("Gallery preview · Approved");
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
                            ));
                        }
                        bar = bar.child(compact_action(
                            "Return for rework",
                            Variant::Secondary,
                            move |_| state.open_sheet(MobileSheet::Rework(cid_rework.clone())),
                        ));
                        col = col.child(gate.child(bar));
                    }

                    let sync_chip = m.sync_mark.map(|mark| {
                        let (lbl, tone) = crate::hosts::sync_chip_spec(mark);
                        status(lbl, tone)
                    });
                    let context_body = m.body.clone();
                    let branch_label = m.branch.clone();
                    let mut ctx = panel().p_3().gap_2().child(
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
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_1p5()
                                    .child(badge(m.stage))
                                    .children(sync_chip)
                                    .children(
                                        branch_label
                                            .map(|b| muted(b).font_family(MONO_FONT).truncate()),
                                    ),
                            )
                            .child(
                                label(move || {
                                    if details_open.get() {
                                        "Hide"
                                    } else {
                                        "Details"
                                    }
                                })
                                .text_color(ACCENT),
                            )
                            .on_press(move |_: &()| details_open.update(|v| *v = !*v)),
                    );
                    if !context_body.is_empty() {
                        ctx = ctx.child(when(
                            move || details_open.get(),
                            move || read_text(context_body.clone()),
                        ));
                    }
                    col = col.child(ctx);

                    let rows = events_memo.get();
                    if rows.is_empty() {
                        col = col.child(
                            panel()
                                .p_4()
                                .child(muted("No conversation or milestones yet.")),
                        );
                    } else {
                        for (ev, _) in rows {
                            col = col.child(timeline_item(ui, ev));
                        }
                    }

                    col.into_element()
                }))),
        )
        .child(
            view()
                .w_full()
                .flex_shrink_0()
                .border_t_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .px_3()
                .pt_2()
                .pb(move || {
                    if is_ios() {
                        px(keyboard_height().0.max(safe_area().bottom.0).max(10.))
                    } else {
                        px(10.)
                    }
                })
                .flex_row()
                .items_center()
                .gap_2()
                .child(field(draft, "Write a reply…").flex_1().min_w(px(0.)))
                .child({
                    let send_cb = send_comment.clone();
                    action(
                        move || if sending.get() { "…" } else { "Send" },
                        Variant::Primary,
                        move |_| send_cb(),
                    )
                    .disabled(move || sending.get() || draft.with(|d| d.trim().is_empty()))
                }),
        )
}

fn card_terminal_view(
    ui: Ui,
    state: MobileState,
    card_id: String,
    model: Memo<Option<CardHeaderModel>>,
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

    view()
        .flex_1()
        .flex_col()
        .min_h(px(0.))
        .child(
            view()
                .w_full()
                .flex_shrink_0()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(BORDER)
                .bg(SURFACE)
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
                                    compact_segment(entry.title(), None, move || is_sel, move |_| {
                                        selected_tid.set(eid_press.clone());
                                        focus_ticket.update(|t| *t += 1);
                                    })
                                }))
                        })),
                )
                .child(compact_action("+ Shell", Variant::Secondary, {
                    let cid = card_id.clone();
                    move |_| {
                        if let Some(m) = model.get_untracked() {
                            crate::terminals::create_in_card(ui, m.project_id, cid.clone(), false);
                        }
                    }
                })),
        )
        .child(dynamic(move || {
            let Some(entry) = active_entry.get() else {
                let cid_new = card_id.clone();
                return view()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .p_5()
                    .gap_3()
                    .child(muted("No active terminal for this Card."))
                    .child(action("Start Shell", Variant::Primary, move |_| {
                        if let Some(m) = model.get_untracked() {
                            crate::terminals::create_in_card(
                                ui,
                                m.project_id,
                                cid_new.clone(),
                                false,
                            );
                        }
                    }))
                    .into_element();
            };

            let focus_req = move || canban_terminal::FocusRequest {
                ticket: focus_ticket.get(),
                enabled: state.sheet.with(Option::is_none),
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
                        "\x1b[1;36m{}\x1b[0m · Mobile Terminal Preview\r\n$ cargo nextest run --workspace\r\nrunning 931 tests... ok\r\n> ",
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
            let cid_move = cid_m.clone();
            let cid_rework = cid_m.clone();
            let cid_deliv = cid_m.clone();
            let rev = m.revision;
            let has_run = m.approval_run.is_some();
            let mut items = vec![
                MenuItem::new("Move to…").on_select(move || {
                    state.open_sheet(MobileSheet::MoveCard(cid_move.clone()));
                }),
                MenuItem::new("Return for rework").on_select(move || {
                    state.open_sheet(MobileSheet::Rework(cid_rework.clone()));
                }),
                MenuItem::new("Deliver…").on_select(move || {
                    deliver_card(ui, &cid_deliv, rev, has_run);
                }),
            ];
            if let Some(br) = m.branch {
                items.push(MenuItem::new("Copy branch").on_select(move || {
                    copy_text(ui, br.clone());
                }));
            }
            items
        }
    };

    let move_cid_desktop = cid_actions.clone();
    let rework_cid_desktop = cid_actions.clone();
    let desktop_header = view()
        .w_full()
        .px_3()
        .py_2p5()
        .border_b_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(compact_action("← Back", Variant::Quiet, move |_| {
            state.pop()
        }))
        .child(
            label(title_fn)
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .flex_1()
                .min_w(px(0.)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(compact_action("Move", Variant::Secondary, move |_| {
                    state.open_sheet(MobileSheet::MoveCard(move_cid_desktop.clone()));
                }))
                .child(compact_action("•••", Variant::Quiet, move |_| {
                    state.open_sheet(MobileSheet::Rework(rework_cid_desktop.clone()));
                })),
        );

    let host_scope = computed(move || model.get().map(|m| m.host_id).filter(|h| !h.is_empty()));

    let mut page = nav_page()
        .key(format!("mobile-card:{card_id}"))
        .title(title_fn)
        .hides_bottom_bar_when_pushed(true)
        .header_visible(move || !state.terminal_fullscreen.get())
        .on_dismiss(move |_: &()| {
            state.terminal_fullscreen.set(false);
            state.pop();
        })
        .header(desktop_header);

    if try_os() == Some(Os::Ios) {
        page = page.header_right_items(move || {
            vec![
                HeaderItem::symbol("ellipsis.circle")
                    .label("Card actions")
                    .menu(more_menu()),
            ]
        });
    }

    page.child(
        view()
            .size_full()
            .flex_col()
            .min_h(px(0.))
            .bg(BACKGROUND)
            .child(
                view()
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(BORDER)
                    .bg(SURFACE)
                    .child(mobile_segmented_control(
                        &["Conversation", "Terminal", "Diff"],
                        move || segment_sig.get().index(),
                        move |idx| {
                            if let Some(next) = CardSegment::from_index(idx) {
                                segment_sig.set(next);
                            }
                        },
                    )),
            )
            .child(dynamic(move || {
                offline_sync_banner(ui, state, host_scope.get())
            }))
            .child(dynamic(move || match segment_sig.get() {
                CardSegment::Conversation => {
                    conversation_view(ui, state, cid_conv.clone(), model).into_element()
                }
                CardSegment::Terminal => {
                    card_terminal_view(ui, state, cid_term.clone(), model).into_element()
                }
                CardSegment::Diff => diff::segment_view(ui, state, cid_diff.clone()).into_element(),
            })),
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
            return muted("Card unavailable").into_element();
        };
        let cid_submit = card_id.clone();
        view()
            .w_full()
            .gap_3()
            .child(label(format!("#{num} {title}")).font_weight(FontWeight::SEMIBOLD))
            .child(muted("Add feedback for the Agent or next iteration."))
            .child(
                nsh_base::input()
                    .value(move || feedback.get())
                    .on_change(move |v: &str| feedback.set(v.to_owned()))
                    .multiline(true)
                    .placeholder("What needs to change…")
                    .min_h(px(100.))
                    .px_3()
                    .py_2()
                    .rounded(px(RADIUS))
                    .bg(INSET)
                    .border_1()
                    .border_color(BORDER)
                    .text_size(px(13.))
                    .text_color(TEXT),
            )
            .child(
                action(
                    move || if busy.get() { "Submitting…" } else { "Return for rework" },
                    Variant::Primary,
                    move |_| {
                        if busy.get_untracked() {
                            return;
                        }
                        trigger_haptic(Haptic::Selection);
                        let note = feedback.get_untracked();
                        if !crate::backend::is_connected() {
                            ui.notify("Gallery preview · Returned for rework");
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
