//! Mobile `Inbox` tab powered by the shared 4-tier `DailyInbox` projection:
//! compact 4-column clickable KPI strip, swipe-to-triage rows (left = Done, right = Defer),
//! Needs Action, project-grouped Active Work, GitHub notifications + pushed Markdown PR/Issue reader,
//! collapsed secondary summaries (Scheduled Runs / Shipped Today / Watching / Earlier),
//! Deferred/Done drawers, and Defer preset sheet.
use super::{
    CardSegment, MobileSheet, MobileState,
    chrome::{
        MOBILE_BODY, disclosure_chevron, empty_state, hairline_divider, hairline_list,
        hairline_list_inset, is_android, is_ios, is_mobile_os, mobile_body, mobile_footnote,
        mobile_inline_button, mobile_subhead, mobile_title, offline_sync_banner, section_header,
        swipe_row, tab_scroll_bottom_pad, trigger_haptic,
    },
};
use crate::{
    inbox::{
        ReadyMergeSummary, ci_indicator, collect_ready_to_merge, comments_indicator, engine_icon,
        format_age_compact, format_duration_short, gh_state_mark, github_avatar,
        state::{
            apply_defer, cutoff_minutes_from_utility, dismiss_github_item, dismiss_github_stack,
            dismiss_items, format_clock_hm, format_defer_until, load_github_item_detail,
            local_tz_offset_secs, refresh_github_notifications, restore_item,
        },
        toggle_focus_filter,
    },
    presenter::*,
};
use canban_notifications::{
    ActiveWorkProjectGroup, DailyInbox, DeferredInboxItem, DoneInboxItem, GitHubProjectStack,
    GitHubStackItem, InboxCardWork, InboxTerminalWork, NeedsYouItem, NeedsYouKind,
    SchedulePulseItem, ShippedTodayItem, UpcomingScheduleItem, defer_presets,
};
use canban_types::{GitHubReference, HerdrStatus};
use canban_ui_kit::{pull_refresh_scroll, tokens::*, *};
use nsh_base::{MenuButtonItem, context_menu, menu_button, nav_page};
use nsh_core::{
    Haptic,
    element::{desktop::MenuItem, header::HeaderItem},
    prelude::*,
};
use serde_json::json;
use std::rc::Rc;

/// Returns total actionable items count driving the `Inbox` bottom tab badge (`1:1` with Needs Action).
pub fn badge_count(ui: Ui) -> usize {
    ui.daily_inbox.with(|i| i.actionable_count)
}

fn active_agent_count(inbox: &DailyInbox) -> usize {
    let mut n = 0usize;
    for item in &inbox.needs_you {
        if matches!(&item.kind, NeedsYouKind::Terminal { terminal } if terminal.is_agent) {
            n += 1;
        }
    }
    for t in &inbox.agent_terminals {
        if matches!(
            t.status,
            HerdrStatus::Working | HerdrStatus::Starting | HerdrStatus::Blocked
        ) {
            n += 1;
        }
    }
    n
}

fn approve_or_deliver_card(ui: Ui, card_id: &str, can_quick_merge: bool) {
    trigger_haptic(Haptic::Selection);
    if !crate::backend::is_connected() {
        let cid = card_id.to_owned();
        ui.snapshot.update(|s| {
            if let Some(c) = s.cards.iter_mut().find(|c| c.id == cid) {
                c.human_gate = None;
                c.attention = None;
            }
        });
        ui.notify(if can_quick_merge {
            "Delivered"
        } else {
            "Approved"
        });
        return;
    }

    let info = ui.snapshot.with_untracked(|s| {
        let card = s.cards.iter().find(|c| c.id == card_id)?.clone();
        let wf = s
            .workflow_runs
            .iter()
            .find(|r| {
                r.active()
                    && r.card_id.as_deref() == Some(card_id)
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
        let next_stage = s
            .projects
            .iter()
            .find(|p| p.id == card.project_id)
            .and_then(|p| {
                let idx = p
                    .workflow
                    .stages
                    .iter()
                    .position(|st| st.id == card.stage)?;
                p.workflow.stages.get(idx + 1).map(|st| st.id.clone())
            });
        Some((card, wf, next_stage))
    });

    let Some((card, wf, next_stage)) = info else {
        return;
    };

    if let Some((run_id, visit, is_deliver)) = wf {
        if is_deliver {
            dispatch(
                ui,
                "workflow.deliver",
                json!({ "card_id": card.id, "revision": card.revision }),
            );
        } else {
            dispatch(
                ui,
                "workflow.decide",
                json!({
                    "run_id": run_id,
                    "visit": visit,
                    "revision": card.revision,
                    "decision": "approve",
                }),
            );
        }
    } else if can_quick_merge {
        crate::card::quick_merge_start(ui, card);
    } else if let Some(stage) = next_stage {
        move_card(ui, &card.id, &stage);
    }
}

fn retry_schedule(ui: Ui, project_id: &str, schedule_id: &str, name: &str) {
    trigger_haptic(Haptic::Selection);
    if !crate::backend::is_connected() {
        ui.notify(format!("Retrying {name}"));
        return;
    }
    dispatch(
        ui,
        "schedule.run",
        json!({ "project_id": project_id, "schedule_id": schedule_id }),
    );
    ui.notify(format!("Triggered {name}"));
}

fn kpi_strip(ui: Ui, inbox: &DailyInbox, ready: &ReadyMergeSummary, active_focus: &str) -> View {
    let waiting_n = inbox.actionable_count;
    let merge_n = ready.total();
    let agents_n = active_agent_count(inbox);
    let shipped_n = inbox.shipped_today.len();

    let cells: [(&'static str, &'static str, usize, ThemeColor); 4] = [
        (
            "waiting",
            "Waiting",
            waiting_n,
            if waiting_n > 0 { ATTENTION } else { MUTED },
        ),
        (
            "merge",
            "Merge",
            merge_n,
            if merge_n > 0 { SUCCESS } else { MUTED },
        ),
        (
            "agents",
            "Agents",
            agents_n,
            if agents_n > 0 { ACCENT } else { MUTED },
        ),
        (
            "shipped",
            "Shipped",
            shipped_n,
            if shipped_n > 0 { TEXT } else { MUTED },
        ),
    ];

    let mut row = view()
        .w_full()
        .flex_row()
        .items_stretch()
        .border_b_1()
        .border_color(BORDER)
        .bg(BACKGROUND);

    for (idx, (key, title, count, num_color)) in cells.into_iter().enumerate() {
        if idx > 0 {
            row = row.child(view().w(px(1.)).my_2().bg(BORDER));
        }
        let selected = active_focus == key;
        row = row.child(
            press_target()
                .flex_1()
                .min_w(px(0.))
                .py_2p5()
                .px_1p5()
                .items_center()
                .justify_center()
                .gap_0p5()
                .relative()
                .bg(if selected { SELECTED } else { CLEAR })
                .active(|s| s.opacity(0.7))
                .child(
                    text(count.to_string())
                        .text_size(px(19.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if selected { ACCENT } else { num_color })
                        .tabular_nums(),
                )
                .child(
                    text(title)
                        .text_size(px(11.5))
                        .font_weight(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if selected { ACCENT } else { MUTED })
                        .truncate(),
                )
                .children(selected.then(|| {
                    view()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(2.))
                        .bg(ACCENT)
                }))
                .on_press(move |_: &()| {
                    trigger_haptic(Haptic::Selection);
                    toggle_focus_filter(ui, key);
                }),
        );
    }
    row
}

fn render_needs_you_item(
    ui: Ui,
    state: MobileState,
    item: NeedsYouItem,
    now_ms: u64,
) -> AnyElement {
    let key = item.key.clone();
    let watermark = item.watermark;
    let age = format_age_compact(item.timestamp_ms, now_ms);

    match item.kind {
        NeedsYouKind::Card {
            card_id,
            card_number,
            card_title,
            project_name,
            terminal_id,
            pr_number,
            pr_checks_summary,
            pr_ci_state,
            headline,
            detail,
            is_review,
            is_blocked,
            unread_seq,
            can_quick_merge,
            ..
        } => {
            let accent_col = if is_blocked {
                DANGER
            } else if is_review || can_quick_merge {
                SUCCESS
            } else {
                ATTENTION
            };
            let primary_lbl = if can_quick_merge {
                "Deliver"
            } else if is_review {
                "Approve"
            } else {
                "Open"
            };

            let cid_open = card_id.clone();
            let cid_primary = card_id.clone();
            let cid_rework = card_id.clone();
            let tid_btn = terminal_id.clone();
            let key_done = key.clone();
            let title_done = card_title.clone();
            let cid_done = card_id.clone();
            let key_defer = key.clone();
            let title_defer = card_title.clone();

            let status_word = if is_blocked {
                "Waiting"
            } else if can_quick_merge {
                "Ready to merge"
            } else if is_review {
                "Review"
            } else {
                "Needs action"
            };
            let ctx_right = if age.is_empty() {
                project_name.clone()
            } else {
                format!("{project_name} · {age}")
            };

            let mut meta_row = view()
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
                        .items_center()
                        .gap_1p5()
                        .child(
                            view()
                                .w(px(6.))
                                .h(px(6.))
                                .rounded_full()
                                .bg(accent_col)
                                .flex_shrink_0(),
                        )
                        .child(
                            text(status_word)
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(accent_col)
                                .flex_shrink_0(),
                        )
                        .children(pr_number.map(|n| {
                            let tone = match pr_ci_state.as_deref() {
                                Some("failure" | "error") => Tone::Danger,
                                Some("success") => Tone::Success,
                                _ => Tone::Neutral,
                            };
                            let lbl = match &pr_checks_summary {
                                Some(sum) if !sum.is_empty() => format!("PR #{n} · {sum}"),
                                _ => format!("PR #{n}"),
                            };
                            status(lbl, tone)
                        })),
                );
            meta_row = meta_row.child(mobile_footnote(ctx_right).tabular_nums().flex_shrink_0());

            let subtitle = detail
                .filter(|d| !d.trim().is_empty() && d != status_word)
                .or_else(|| {
                    (!headline.trim().is_empty() && headline != status_word).then_some(headline)
                })
                .unwrap_or_default();

            let tap_body = press_target()
                .w_full()
                .items_start()
                .gap_1()
                .active(|s| s.opacity(0.72))
                .child(meta_row)
                .child(mobile_title(format!("#{card_number} {card_title}")).number_of_lines(2))
                .children(
                    (!subtitle.trim().is_empty())
                        .then(|| mobile_subhead(subtitle).number_of_lines(2)),
                )
                .on_press(move |_: &()| {
                    trigger_haptic(Haptic::Selection);
                    if unread_seq > 0 {
                        crate::overview::mark_card_read(ui, cid_open.clone(), unread_seq);
                    }
                    state.push_card(cid_open.clone(), CardSegment::Conversation);
                });

            let mut actions = view().w_full().flex_row().items_center().gap_2().pt_1();
            if is_review || can_quick_merge || is_blocked {
                actions = actions
                    .child(
                        mobile_inline_button("Rework", Variant::Secondary, move |_| {
                            state.open_sheet(MobileSheet::Rework(cid_rework.clone()));
                        })
                        .flex_1(),
                    )
                    .child(
                        mobile_inline_button(primary_lbl, Variant::Primary, move |_| {
                            if primary_lbl == "Open" {
                                state.push_card(cid_primary.clone(), CardSegment::Conversation);
                            } else {
                                approve_or_deliver_card(ui, &cid_primary, can_quick_merge);
                            }
                        })
                        .flex_1(),
                    );
            } else {
                actions = actions.child(
                    mobile_inline_button("Open", Variant::Secondary, move |_| {
                        state.push_card(cid_primary.clone(), CardSegment::Conversation);
                    })
                    .flex_1(),
                );
            }
            if let Some(tid) = tid_btn {
                actions = actions.child(
                    press_target()
                        .w(px(42.))
                        .h(px(42.))
                        .rounded(px(10.))
                        .bg(INSET)
                        .items_center()
                        .justify_center()
                        .active(|s| s.opacity(0.7))
                        .accessibility_label(Some(SharedString::from("Open terminal")))
                        .child(icon_tinted(Icon::Terminal, ACCENT).w(px(17.)).h(px(17.)))
                        .on_press(move |_: &()| {
                            trigger_haptic(Haptic::Selection);
                            state.push_terminal(tid.clone());
                        }),
                );
            }

            let card_body = view()
                .w_full()
                .px_4()
                .py_3p5()
                .gap_2p5()
                .border_b_1()
                .border_color(BORDER)
                .child(tap_body)
                .child(actions);

            let do_done: Rc<dyn Fn()> = Rc::new(move || {
                if unread_seq > 0 {
                    crate::overview::mark_card_read(ui, cid_done.clone(), unread_seq);
                }
                dismiss_items(
                    ui,
                    vec![(key_done.clone(), watermark)],
                    format!("Done · {title_done}"),
                );
            });
            let do_defer: Rc<dyn Fn()> = Rc::new(move || {
                state.open_defer(vec![(key_defer.clone(), watermark)], title_defer.clone());
            });

            let swiped = swipe_row(
                card_body,
                "Defer",
                Icon::Clock,
                Tone::Attention,
                Some(do_defer.clone()),
                "Done",
                Icon::Check,
                Tone::Success,
                Some(do_done.clone()),
            );

            if !is_mobile_os() {
                return swiped.into_element();
            }

            let mut menu_items = vec![MenuButtonItem::new("open", "Open Card").icon("bubble.left")];
            if terminal_id.is_some() {
                menu_items.push(MenuButtonItem::new("terminal", "Open Terminal").icon("terminal"));
            }
            menu_items.push(MenuButtonItem::new("diff", "View Diff").icon("doc.text"));
            menu_items.push(MenuButtonItem::new("defer", "Defer…").icon("clock"));
            menu_items.push(MenuButtonItem::new("done", "Mark Done").icon("checkmark.circle"));

            let cid_menu = card_id;
            let tid_menu = terminal_id;
            context_menu()
                .w_full()
                .items(menu_items)
                .trigger(swiped)
                .on_select(move |choice: &SharedString| {
                    trigger_haptic(Haptic::Selection);
                    match choice.as_ref() {
                        "open" => state.push_card(cid_menu.clone(), CardSegment::Conversation),
                        "diff" => state.push_card(cid_menu.clone(), CardSegment::Diff),
                        "terminal" => {
                            if let Some(tid) = &tid_menu {
                                state.push_terminal(tid.clone());
                            }
                        }
                        "defer" => do_defer(),
                        "done" => do_done(),
                        _ => {}
                    }
                })
                .into_element()
        }
        NeedsYouKind::ScheduleFailed {
            schedule_id,
            schedule_name,
            project_id,
            project_name,
            summary,
            error,
            terminal_id,
            ..
        } => {
            let err_msg = error.or(summary).unwrap_or_else(|| "Run failed".to_owned());
            let pid_retry = project_id.clone();
            let sid_retry = schedule_id.clone();
            let name_retry = schedule_name.clone();
            let tid_open = terminal_id.clone();

            let mut actions = view()
                .w_full()
                .flex_row()
                .items_center()
                .gap_2()
                .pt_1()
                .child(
                    mobile_inline_button("Retry", Variant::Primary, move |_| {
                        retry_schedule(ui, &pid_retry, &sid_retry, &name_retry);
                    })
                    .flex_1(),
                );
            if let Some(tid) = tid_open {
                actions = actions.child(
                    mobile_inline_button("Terminal", Variant::Secondary, move |_| {
                        state.push_terminal(tid.clone());
                    })
                    .flex_1(),
                );
            }

            let card_body = view()
                .w_full()
                .px_4()
                .py_3p5()
                .gap_2p5()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    view()
                        .w_full()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .flex_1()
                                .min_w(px(0.))
                                .child(
                                    view()
                                        .w(px(6.))
                                        .h(px(6.))
                                        .rounded_full()
                                        .bg(DANGER)
                                        .flex_shrink_0(),
                                )
                                .child(
                                    text("Failed")
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(DANGER)
                                        .flex_shrink_0(),
                                )
                                .child(
                                    mobile_footnote(format!("Schedule · {project_name}"))
                                        .truncate(),
                                ),
                        )
                        .children(
                            (!age.is_empty())
                                .then(|| mobile_footnote(age).tabular_nums().flex_shrink_0()),
                        ),
                )
                .child(mobile_title(schedule_name.clone()).number_of_lines(2))
                .child(mobile_subhead(err_msg).number_of_lines(2))
                .child(actions);

            let key_done = key.clone();
            let name_done = schedule_name.clone();
            let key_defer = key;
            let name_defer = schedule_name;
            swipe_row(
                card_body,
                "Defer",
                Icon::Clock,
                Tone::Attention,
                Some(Rc::new(move || {
                    state.open_defer(vec![(key_defer.clone(), watermark)], name_defer.clone());
                })),
                "Done",
                Icon::Check,
                Tone::Success,
                Some(Rc::new(move || {
                    dismiss_items(
                        ui,
                        vec![(key_done.clone(), watermark)],
                        format!("Done · {name_done}"),
                    );
                })),
            )
            .into_element()
        }
        NeedsYouKind::Terminal { terminal } => {
            let tid_open = terminal.terminal_id.clone();
            let tid_btn = terminal.terminal_id.clone();
            let cid_opt = terminal.card_id.clone();
            let accent_col = if matches!(
                terminal.status,
                HerdrStatus::Error | HerdrStatus::Disconnected
            ) {
                DANGER
            } else {
                ATTENTION
            };
            let ic = if terminal.is_agent {
                engine_icon(terminal.agent_engine.as_deref())
            } else {
                Icon::Terminal
            };
            let detail = terminal
                .waiting_title
                .clone()
                .or_else(|| terminal.status_detail.clone())
                .unwrap_or_else(|| "Input needed".to_owned());
            let ctx = match (&terminal.project_name, &terminal.branch) {
                (Some(p), Some(b)) => format!("{p} · {b}"),
                (Some(p), None) => p.clone(),
                (None, Some(b)) => b.clone(),
                (None, None) => terminal.host_label.clone(),
            };

            let term_status_word = if matches!(
                terminal.status,
                HerdrStatus::Error | HerdrStatus::Disconnected
            ) {
                "Error"
            } else {
                "Waiting"
            };
            let card_body = view()
                .w_full()
                .px_4()
                .py_3p5()
                .gap_2p5()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    press_target()
                        .w_full()
                        .gap_1()
                        .active(|s| s.opacity(0.72))
                        .child(
                            view()
                                .w_full()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_1p5()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .child(
                                            view()
                                                .w(px(6.))
                                                .h(px(6.))
                                                .rounded_full()
                                                .bg(accent_col)
                                                .flex_shrink_0(),
                                        )
                                        .child(
                                            text(term_status_word)
                                                .text_size(px(12.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(accent_col)
                                                .flex_shrink_0(),
                                        )
                                        .child(mobile_footnote(ctx).truncate()),
                                )
                                .children(
                                    (!age.is_empty()).then(|| {
                                        mobile_footnote(age).tabular_nums().flex_shrink_0()
                                    }),
                                ),
                        )
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(if terminal.is_agent && terminal.agent_engine.is_some() {
                                    icon_colored(ic, 16.).into_element()
                                } else {
                                    icon_tinted(ic, accent_col)
                                        .w(px(15.))
                                        .h(px(15.))
                                        .into_element()
                                })
                                .child(mobile_title(terminal.title.clone()).number_of_lines(2)),
                        )
                        .child(mobile_subhead(detail).number_of_lines(2))
                        .on_press(move |_: &()| {
                            trigger_haptic(Haptic::Selection);
                            if let Some(cid) = &cid_opt {
                                state.push_card(cid.clone(), CardSegment::Terminal);
                            } else {
                                state.push_terminal(tid_open.clone());
                            }
                        }),
                )
                .child(mobile_inline_button(
                    "Open Terminal",
                    Variant::Secondary,
                    move |_| state.push_terminal(tid_btn.clone()),
                ));

            let key_done = key.clone();
            let title_done = terminal.title.clone();
            let key_defer = key;
            let title_defer = terminal.title;
            swipe_row(
                card_body,
                "Defer",
                Icon::Clock,
                Tone::Attention,
                Some(Rc::new(move || {
                    state.open_defer(vec![(key_defer.clone(), watermark)], title_defer.clone());
                })),
                "Done",
                Icon::Check,
                Tone::Success,
                Some(Rc::new(move || {
                    dismiss_items(
                        ui,
                        vec![(key_done.clone(), watermark)],
                        format!("Hidden · {title_done}"),
                    );
                })),
            )
            .into_element()
        }
        NeedsYouKind::GitHub {
            item: gh,
            project_name,
            ..
        } => render_github_row_item(ui, state, *gh, Some(project_name), now_ms, true),
    }
}

fn render_github_row_item(
    ui: Ui,
    state: MobileState,
    gh: GitHubStackItem,
    project_override: Option<String>,
    now_ms: u64,
    prominent: bool,
) -> AnyElement {
    let detail = ui
        .inbox
        .github_item_details
        .with(|m| m.get(&gh.key).cloned());
    let merged =
        detail.as_ref().is_some_and(|d| d.merged) || gh.pr_state.as_deref() == Some("merged");
    let draft = detail.as_ref().is_some_and(|d| d.draft) || gh.pr_state.as_deref() == Some("draft");
    let pr_state = detail
        .as_ref()
        .map(|d| d.state.as_str())
        .or(gh.pr_state.as_deref());
    let ci_state = detail
        .as_ref()
        .and_then(|d| d.ci_state.as_deref())
        .or(gh.ci_state.as_deref());
    let (ci_passed, ci_total) = match detail.as_ref().filter(|d| d.ci_total > 0) {
        Some(d) => (d.ci_passed, d.ci_total),
        None => (gh.ci_passed, gh.ci_total),
    };
    let failed_n = detail
        .as_ref()
        .filter(|d| !d.failed_checks.is_empty())
        .map(|d| d.failed_checks.len())
        .unwrap_or(gh.failed_checks.len());
    let comments = detail
        .as_ref()
        .and_then(|d| d.comments_count)
        .or(gh.comments_count);
    let age = format_age_compact(gh.timestamp_ms, now_ms);

    let repo_short = project_override.unwrap_or_else(|| {
        gh.repository
            .rsplit('/')
            .next()
            .unwrap_or(&gh.repository)
            .to_owned()
    });
    let num_str = gh.number.map(|n| format!("#{n}")).unwrap_or_default();

    let gh_open = gh.clone();
    let gh_done = gh.clone();
    let key_defer = gh.key.clone();
    let wm_defer = gh.watermark;
    let title_defer = gh.title.clone();

    let mut meta = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_1p5()
        .child(mobile_footnote(format!("{repo_short} {num_str}")).tabular_nums());
    if !gh.reason.is_empty() {
        meta = meta.child(mobile_footnote(format!("· {}", gh.reason)));
    }
    if let Some(ci) = ci_indicator(ci_state, ci_passed, ci_total, failed_n) {
        meta = meta.child(ci);
    }
    if let Some(c) = comments
        && c > 0
    {
        meta = meta.child(comments_indicator(c));
    }

    let inner = press_target()
        .w_full()
        .px_4()
        .py_3()
        .flex_row()
        .items_start()
        .gap_3()
        .active(|s| s.opacity(0.72))
        .child(
            view()
                .pt_0p5()
                .flex_shrink_0()
                .child(gh_state_mark(&gh.kind, pr_state, merged, draft)),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .gap_1()
                .child(
                    view()
                        .w_full()
                        .flex_row()
                        .items_start()
                        .justify_between()
                        .gap_2()
                        .child(
                            text(gh.title.clone())
                                .text_size(px(15.5))
                                .line_height(px(21.))
                                .font_weight(if prominent {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(TEXT)
                                .number_of_lines(2)
                                .flex_1()
                                .min_w(px(0.)),
                        )
                        .children(
                            (!age.is_empty())
                                .then(|| mobile_footnote(age).tabular_nums().flex_shrink_0()),
                        ),
                )
                .child(meta),
        )
        .child(view().pt_1().child(disclosure_chevron()))
        .on_press(move |_: &()| {
            trigger_haptic(Haptic::Selection);
            state.push_github_item(gh_open.clone());
        });

    let wrapped: AnyElement = if prominent {
        view()
            .w_full()
            .border_b_1()
            .border_color(BORDER)
            .child(inner)
            .into_element()
    } else {
        inner.into_element()
    };

    swipe_row(
        wrapped,
        "Defer",
        Icon::Clock,
        Tone::Attention,
        Some(Rc::new(move || {
            state.open_defer(vec![(key_defer.clone(), wm_defer)], title_defer.clone());
        })),
        "Done",
        Icon::Check,
        Tone::Success,
        Some(Rc::new(move || {
            dismiss_github_item(ui, gh_done.clone());
        })),
    )
    .into_element()
}

fn render_active_terminal_row(ui: Ui, state: MobileState, term: InboxTerminalWork) -> AnyElement {
    let tid = term.terminal_id.clone();
    let cid_opt = term.card_id.clone();
    let ic = if term.is_agent {
        engine_icon(term.agent_engine.as_deref())
    } else {
        Icon::Terminal
    };

    let (status_el, icon_tint): (AnyElement, ThemeColor) = match term.status {
        HerdrStatus::Working | HerdrStatus::Starting | HerdrStatus::Queued => {
            (live_dot(ACCENT).into_element(), ACCENT)
        }
        HerdrStatus::Blocked => (
            icon_tinted(Icon::ShieldAlert, ATTENTION)
                .w(px(14.))
                .h(px(14.))
                .into_element(),
            ATTENTION,
        ),
        HerdrStatus::Error | HerdrStatus::Disconnected => (
            icon_tinted(Icon::TriangleAlert, DANGER)
                .w(px(14.))
                .h(px(14.))
                .into_element(),
            DANGER,
        ),
        HerdrStatus::Done => (
            icon_tinted(Icon::Check, SUCCESS)
                .w(px(14.))
                .h(px(14.))
                .into_element(),
            SUCCESS,
        ),
        HerdrStatus::PlainShell => match term.shell_exit_code {
            None if term.shell_command.is_some() => (status_dot(ACCENT).into_element(), ACCENT),
            Some(0) => (
                icon_tinted(Icon::Check, SUCCESS)
                    .w(px(13.))
                    .h(px(13.))
                    .into_element(),
                MUTED,
            ),
            Some(code) => (
                text(format!("exit {code}"))
                    .text_size(px(12.))
                    .font_family(MONO_FONT)
                    .text_color(DANGER)
                    .into_element(),
                DANGER,
            ),
            None => (view().into_element(), MUTED),
        },
        _ => (view().into_element(), MUTED),
    };

    let mut telemetry = Vec::new();
    if let Some(name) = &term.agent_name {
        telemetry.push(name.clone());
    }
    if let Some(br) = &term.branch
        && !br.is_empty()
    {
        telemetry.push(crate::components::git_workspace::compact(br, 20));
    }
    if term.additions > 0 || term.deletions > 0 {
        telemetry.push(format!("+{} -{}", term.additions, term.deletions));
    }
    if let Some(cmd) = &term.shell_command
        && !cmd.is_empty()
        && !term.is_agent
    {
        telemetry.push(cmd.clone());
    }
    let subtitle = if !telemetry.is_empty() {
        telemetry.join(" · ")
    } else if let Some(detail) = term.status_detail.filter(|s| !s.is_empty()) {
        detail
    } else {
        term.host_label.clone()
    };

    let started_at = term.started_at;
    let display_title = if let (Some(n), Some(ct)) = (term.card_number, &term.card_title) {
        format!("#{n} {ct}")
    } else {
        term.title.clone()
    };

    press_target()
        .w_full()
        .min_h(px(54.))
        .px_4()
        .py_2p5()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .active(|s| s.opacity(0.72))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .flex_1()
                .min_w(px(0.))
                .child(
                    icon_tinted(ic, icon_tint)
                        .w(px(18.))
                        .h(px(18.))
                        .flex_shrink_0(),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_0p5()
                        .child(
                            text(display_title)
                                .text_size(px(15.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT)
                                .truncate(),
                        )
                        .child(mobile_footnote(subtitle).truncate()),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .child(status_el)
                .children(started_at.map(|ts| {
                    text(move || format_duration_short(ts, ui.runtime_clock.get()))
                        .text_size(px(12.5))
                        .text_color(MUTED)
                        .tabular_nums()
                }))
                .child(disclosure_chevron()),
        )
        .on_press(move |_: &()| {
            trigger_haptic(Haptic::Selection);
            if let Some(cid) = &cid_opt {
                state.push_card(cid.clone(), CardSegment::Terminal);
            } else {
                state.push_terminal(tid.clone());
            }
        })
        .into_element()
}

fn render_open_card_row(
    ui: Ui,
    state: MobileState,
    card: InboxCardWork,
    now_ms: u64,
) -> AnyElement {
    let cid = card.card_id.clone();
    let key_done = card.key.clone();
    let key_defer = card.key.clone();
    let wm = card.watermark;
    let title_done = card.card_title.clone();
    let title_defer = card.card_title.clone();
    let age = format_age_compact(card.last_activity_at, now_ms);

    let mut meta_parts = vec![card.stage.clone()];
    if let Some(br) = &card.branch
        && !br.is_empty()
    {
        meta_parts.push(crate::components::git_workspace::compact(br, 20));
    }
    if let Some(pr) = &card.pr_summary
        && !pr.is_empty()
    {
        meta_parts.push(pr.clone());
    }

    let row = press_target()
        .w_full()
        .min_h(px(52.))
        .px_4()
        .py_2p5()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .active(|s| s.opacity(0.72))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .flex_1()
                .min_w(px(0.))
                .child(
                    text(format!("#{}", card.card_number))
                        .text_size(px(12.5))
                        .font_family(MONO_FONT)
                        .text_color(MUTED)
                        .tabular_nums()
                        .flex_shrink_0(),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_0p5()
                        .child(
                            text(card.card_title.clone())
                                .text_size(px(15.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT)
                                .truncate(),
                        )
                        .child(mobile_footnote(meta_parts.join(" · ")).truncate()),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .children((!age.is_empty()).then(|| mobile_footnote(age).tabular_nums()))
                .child(disclosure_chevron()),
        )
        .on_press(move |_: &()| {
            trigger_haptic(Haptic::Selection);
            state.push_card(cid.clone(), CardSegment::Conversation);
        });

    swipe_row(
        row,
        "Defer",
        Icon::Clock,
        Tone::Attention,
        Some(Rc::new(move || {
            state.open_defer(vec![(key_defer.clone(), wm)], title_defer.clone());
        })),
        "Done",
        Icon::Check,
        Tone::Success,
        Some(Rc::new(move || {
            dismiss_items(
                ui,
                vec![(key_done.clone(), wm)],
                format!("Done · {title_done}"),
            );
        })),
    )
    .into_element()
}

fn render_active_work_group(
    ui: Ui,
    state: MobileState,
    group: ActiveWorkProjectGroup,
    now_ms: u64,
    agents_only: bool,
) -> Option<View> {
    let mut rows: Vec<AnyElement> = Vec::new();
    for term in group.agent_terminals {
        rows.push(render_active_terminal_row(ui, state, term));
    }
    if !agents_only {
        for card in group.open_cards {
            rows.push(render_open_card_row(ui, state, card, now_ms));
        }
        for shell in group.plain_shells {
            rows.push(render_active_terminal_row(ui, state, shell));
        }
    }
    if rows.is_empty() {
        return None;
    }

    let heading = match &group.host_label {
        Some(h) if !h.is_empty() => format!("{} · {}", group.display_name, h),
        _ => group.display_name.clone(),
    };
    let trailing = (group.live_agent_count > 0).then(|| {
        view()
            .flex_row()
            .items_center()
            .gap_1()
            .child(live_dot(ACCENT))
            .child(mobile_footnote(format!("{} live", group.live_agent_count)).tabular_nums())
            .into_element()
    });

    Some(
        view()
            .w_full()
            .child(
                view()
                    .px_4()
                    .pt_2()
                    .child(section_header(heading.to_uppercase(), trailing)),
            )
            .child(hairline_list_inset(rows, 44.0)),
    )
}

fn render_github_stack(ui: Ui, state: MobileState, stack: GitHubProjectStack, now_ms: u64) -> View {
    let stack_clear = stack.clone();
    let clear_btn = press_target()
        .min_h(px(28.))
        .px_1p5()
        .items_center()
        .justify_center()
        .active(|s| s.opacity(0.65))
        .child(
            text("Clear")
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ACCENT),
        )
        .on_press(move |_: &()| {
            trigger_haptic(Haptic::Selection);
            dismiss_github_stack(ui, stack_clear.clone());
        })
        .into_element();

    let rows: Vec<AnyElement> = stack
        .items
        .into_iter()
        .map(|item| render_github_row_item(ui, state, item, None, now_ms, false))
        .collect();

    view()
        .w_full()
        .child(view().px_4().pt_2().child(section_header(
            format!("{} ({})", stack.display_name.to_uppercase(), rows.len()),
            Some(clear_btn),
        )))
        .child(hairline_list_inset(rows, 44.0))
}

fn render_collapsible_summary(
    label_text: String,
    count: usize,
    expanded: Signal<bool>,
    body: impl Fn() -> AnyElement + 'static,
) -> View {
    view()
        .w_full()
        .child(
            press_target()
                .w_full()
                .min_h(px(48.))
                .px_4()
                .py_2p5()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .active(|s| s.opacity(0.7))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            text(label_text)
                                .text_size(px(14.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT),
                        )
                        .child(
                            text(count.to_string())
                                .text_size(px(13.))
                                .text_color(MUTED)
                                .tabular_nums(),
                        ),
                )
                .child(dynamic(move || {
                    icon_tinted(
                        if expanded.get() {
                            Icon::ChevronDown
                        } else {
                            Icon::Forward
                        },
                        MUTED,
                    )
                    .w(px(15.))
                    .h(px(15.))
                }))
                .on_press(move |_: &()| {
                    trigger_haptic(Haptic::Selection);
                    expanded.update(|v| *v = !*v);
                }),
        )
        .child(when(move || expanded.get(), body))
}

fn render_schedule_rows(
    ui: Ui,
    state: MobileState,
    pulse: Vec<SchedulePulseItem>,
    upcoming: Vec<UpcomingScheduleItem>,
) -> AnyElement {
    let mut rows: Vec<AnyElement> = Vec::new();
    for run in pulse {
        let is_failed = matches!(run.state.as_str(), "failed" | "error" | "timed_out");
        let tone = if is_failed {
            Tone::Danger
        } else if run.state == "running" {
            Tone::Info
        } else {
            Tone::Success
        };
        let pid = run.project_id.clone();
        let sid = run.schedule_id.clone();
        let sname = run.schedule_name.clone();
        let cid_opt = run.card_id.clone();
        let sub = format!(
            "{} · {}",
            run.project_name,
            run.summary
                .as_deref()
                .or(run.error.as_deref())
                .unwrap_or(&run.state)
        );
        rows.push(
            view()
                .w_full()
                .min_h(px(50.))
                .px_4()
                .py_2p5()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    press_target()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_0p5()
                        .active(|s| s.opacity(0.7))
                        .child(
                            text(run.schedule_name.clone())
                                .text_size(px(15.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT)
                                .truncate(),
                        )
                        .child(mobile_footnote(sub).truncate())
                        .on_press(move |_: &()| {
                            if let Some(cid) = &cid_opt {
                                trigger_haptic(Haptic::Selection);
                                state.push_card(cid.clone(), CardSegment::Conversation);
                            }
                        }),
                )
                .child(if is_failed {
                    press_target()
                        .h(px(30.))
                        .px_2p5()
                        .rounded(px(8.))
                        .bg(SELECTED)
                        .items_center()
                        .justify_center()
                        .child(
                            text("Retry")
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ACCENT),
                        )
                        .on_press(move |_: &()| retry_schedule(ui, &pid, &sid, &sname))
                        .into_element()
                } else {
                    status(run.state, tone).into_element()
                })
                .into_element(),
        );
    }
    for up in upcoming {
        let next_hm = format_clock_hm(up.next_run_at);
        let pid = up.project_id.clone();
        let sid = up.schedule_id.clone();
        let sname = up.schedule_name.clone();
        rows.push(
            view()
                .w_full()
                .min_h(px(48.))
                .px_4()
                .py_2()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_0p5()
                        .child(
                            text(up.schedule_name)
                                .text_size(px(14.5))
                                .text_color(TEXT)
                                .truncate(),
                        )
                        .child(mobile_footnote(format!(
                            "{} · next {next_hm}",
                            up.project_name
                        ))),
                )
                .child(
                    press_target()
                        .h(px(28.))
                        .px_2p5()
                        .rounded(px(7.))
                        .bg(INSET)
                        .items_center()
                        .justify_center()
                        .child(
                            text("Run")
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ACCENT),
                        )
                        .on_press(move |_: &()| retry_schedule(ui, &pid, &sid, &sname)),
                )
                .into_element(),
        );
    }
    hairline_list_inset(rows, 16.0).into_element()
}

fn render_shipped_rows(
    ui: Ui,
    state: MobileState,
    items: Vec<ShippedTodayItem>,
    now_ms: u64,
) -> AnyElement {
    let rows: Vec<AnyElement> = items
        .into_iter()
        .map(|s| {
            let cid = s.card_id.clone();
            let url = s.url.clone();
            let age = format_age_compact(s.shipped_at, now_ms);
            let sub = if s.is_pr {
                match (&s.repository, s.pr_number) {
                    (Some(r), Some(n)) => format!("{} #{n}", r.rsplit('/').next().unwrap_or(r)),
                    _ => s.project_name.clone(),
                }
            } else if let Some(n) = s.card_number {
                format!("{} · #{n}", s.project_name)
            } else {
                s.project_name.clone()
            };
            press_target()
                .w_full()
                .min_h(px(48.))
                .px_4()
                .py_2()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .active(|st| st.opacity(0.72))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2p5()
                        .flex_1()
                        .min_w(px(0.))
                        .child(
                            icon_tinted(if s.is_pr { Icon::GitHub } else { Icon::Check }, SUCCESS)
                                .w(px(15.))
                                .h(px(15.))
                                .flex_shrink_0(),
                        )
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.))
                                .gap_0p5()
                                .child(
                                    text(s.title)
                                        .text_size(px(14.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(TEXT)
                                        .truncate(),
                                )
                                .child(mobile_footnote(sub).truncate()),
                        ),
                )
                .child(mobile_footnote(age).tabular_nums().flex_shrink_0())
                .on_press(move |_: &()| {
                    trigger_haptic(Haptic::Selection);
                    if let Some(c) = &cid {
                        state.push_card(c.clone(), CardSegment::Conversation);
                    } else if let Some(u) = &url {
                        let target = u.clone();
                        ui.spawn(async move {
                            let _ = open_url(&target).await;
                        });
                    }
                })
                .into_element()
        })
        .collect();
    hairline_list_inset(rows, 40.0).into_element()
}

fn render_deferred_drawer(ui: Ui, items: Vec<DeferredInboxItem>) -> AnyElement {
    if items.is_empty() {
        return empty_state(Icon::Clock, Tone::Neutral, "No deferred items", "", None)
            .into_element();
    }
    let rows: Vec<AnyElement> = items
        .into_iter()
        .map(|item| {
            let k = item.key.clone();
            let until = format_defer_until(item.defer_until);
            view()
                .w_full()
                .min_h(px(56.))
                .px_4()
                .py_2p5()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_0p5()
                        .child(
                            text(item.title)
                                .text_size(px(15.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT)
                                .truncate(),
                        )
                        .child(
                            mobile_footnote(format!("{} · until {until}", item.context)).truncate(),
                        ),
                )
                .child(
                    press_target()
                        .h(px(30.))
                        .px_2p5()
                        .rounded(px(8.))
                        .bg(SELECTED)
                        .items_center()
                        .justify_center()
                        .child(
                            text("Restore")
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ACCENT),
                        )
                        .on_press(move |_: &()| {
                            trigger_haptic(Haptic::Selection);
                            restore_item(ui, k.clone());
                        }),
                )
                .into_element()
        })
        .collect();
    hairline_list_inset(rows, 16.0).into_element()
}

fn render_done_drawer(ui: Ui, items: Vec<DoneInboxItem>, now_ms: u64) -> AnyElement {
    if items.is_empty() {
        return empty_state(Icon::Check, Tone::Neutral, "Nothing done yet", "", None)
            .into_element();
    }
    let rows: Vec<AnyElement> = items
        .into_iter()
        .map(|item| {
            let k = item.key.clone();
            let age = format_age_compact(item.completed_at, now_ms);
            view()
                .w_full()
                .min_h(px(54.))
                .px_4()
                .py_2p5()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_0p5()
                        .child(
                            text(item.title)
                                .text_size(px(15.))
                                .text_color(MUTED)
                                .truncate(),
                        )
                        .child(mobile_footnote(format!("{} · {age}", item.context)).truncate()),
                )
                .children(item.can_restore.then(|| {
                    press_target()
                        .h(px(28.))
                        .px_2p5()
                        .rounded(px(8.))
                        .bg(INSET)
                        .items_center()
                        .justify_center()
                        .child(
                            text("Restore")
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ACCENT),
                        )
                        .on_press(move |_: &()| {
                            trigger_haptic(Haptic::Selection);
                            restore_item(ui, k.clone());
                        })
                }))
                .into_element()
        })
        .collect();
    hairline_list_inset(rows, 16.0).into_element()
}

pub(crate) fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let scroll_ref = element_ref();
    let schedules_open = signal(false);
    let shipped_open = signal(false);
    let watching_open = signal(false);
    let earlier_open = signal(false);

    let body = pull_refresh_scroll(
        scroll().w_full().bg(BACKGROUND).child(
            view()
                .w_full()
                .pb(px(tab_scroll_bottom_pad()))
                .child(
                    view()
                        .w_full()
                        .px_4()
                        .child(offline_sync_banner(ui, state, None)),
                )
                .child(dynamic(move || {
                    let inbox = ui.daily_inbox.get();
                    let drawer = ui.inbox.drawer.get();
                    let focus = ui.inbox.focus_filter.get();
                    let gh_details = ui.inbox.github_item_details.get();
                    let now_ms = ui.runtime_clock.get().max(canban_types::now());

                    if drawer == "deferred" {
                        return render_deferred_drawer(ui, inbox.deferred_items.clone());
                    }
                    if drawer == "done" {
                        return render_done_drawer(ui, inbox.done_today_items.clone(), now_ms);
                    }

                    let ready = collect_ready_to_merge(&inbox, &gh_details);
                    let mut col = view().w_full().child(kpi_strip(ui, &inbox, &ready, &focus));

                    // Focused KPI filter views
                    if !focus.is_empty() {
                        match focus.as_str() {
                            "waiting" => {
                                if inbox.needs_you.is_empty() {
                                    col = col.child(empty_state(
                                        Icon::Check,
                                        Tone::Success,
                                        "Nothing waiting on you",
                                        "",
                                        None,
                                    ));
                                } else {
                                    for item in inbox.needs_you.clone() {
                                        col = col
                                            .child(render_needs_you_item(ui, state, item, now_ms));
                                    }
                                }
                            }
                            "merge" => {
                                if ready.total() == 0 {
                                    col = col.child(empty_state(
                                        Icon::Check,
                                        Tone::Neutral,
                                        "Nothing ready to merge",
                                        "",
                                        None,
                                    ));
                                } else {
                                    for item in ready.needs_you_items {
                                        col = col
                                            .child(render_needs_you_item(ui, state, item, now_ms));
                                    }
                                    let mut extra: Vec<AnyElement> = Vec::new();
                                    for card in ready.open_cards {
                                        extra.push(render_open_card_row(ui, state, card, now_ms));
                                    }
                                    for gh in ready.gh_items {
                                        extra.push(render_github_row_item(
                                            ui, state, gh, None, now_ms, false,
                                        ));
                                    }
                                    if !extra.is_empty() {
                                        col = col.child(hairline_list_inset(extra, 44.0));
                                    }
                                }
                            }
                            "agents" => {
                                let mut any = false;
                                for g in inbox.active_work_groups.clone() {
                                    if let Some(sec) =
                                        render_active_work_group(ui, state, g, now_ms, true)
                                    {
                                        any = true;
                                        col = col.child(sec);
                                    }
                                }
                                if !any {
                                    col = col.child(empty_state(
                                        Icon::Agent,
                                        Tone::Neutral,
                                        "No active agents",
                                        "",
                                        None,
                                    ));
                                }
                            }
                            "shipped" => {
                                if inbox.shipped_today.is_empty() {
                                    col = col.child(empty_state(
                                        Icon::Check,
                                        Tone::Neutral,
                                        "Nothing shipped today",
                                        "",
                                        None,
                                    ));
                                } else {
                                    col = col.child(render_shipped_rows(
                                        ui,
                                        state,
                                        inbox.shipped_today.clone(),
                                        now_ms,
                                    ));
                                }
                            }
                            _ => {}
                        }
                        return col.into_element();
                    }

                    // Default Today Briefing flow
                    let has_primary = !inbox.needs_you.is_empty()
                        || !inbox.active_work_groups.is_empty()
                        || !inbox.github_stacks.is_empty();
                    let sched_count = inbox.schedule_pulse.len() + inbox.upcoming_schedules.len();
                    let shipped_count = inbox.shipped_today.len();
                    let watching_count: usize =
                        inbox.github_watching.iter().map(|s| s.items.len()).sum();
                    let earlier_count = inbox.earlier_github.len() + inbox.carryover_cards.len();
                    let has_secondary =
                        sched_count + shipped_count + watching_count + earlier_count > 0;

                    if !has_primary && !has_secondary {
                        return col
                            .child(empty_state(
                                Icon::Check,
                                Tone::Success,
                                "All caught up",
                                "",
                                None,
                            ))
                            .into_element();
                    }

                    // 1. Needs Action (flat editorial hairline rows directly on canvas)
                    if !inbox.needs_you.is_empty() {
                        let mut list = view().w_full().border_t_1().border_color(BORDER);
                        for item in inbox.needs_you.clone() {
                            list = list.child(render_needs_you_item(ui, state, item, now_ms));
                        }
                        let sec = view()
                            .w_full()
                            .pt_3()
                            .child(view().px_4().child(section_header("NEEDS ACTION", None)))
                            .child(list);
                        col = col.child(sec);
                    }

                    // 2. Active Work (grouped by Project)
                    if !inbox.active_work_groups.is_empty() {
                        let mut sec = view().w_full().pt_3().gap_2();
                        for group in inbox.active_work_groups.clone() {
                            if let Some(g_el) =
                                render_active_work_group(ui, state, group, now_ms, false)
                            {
                                sec = sec.child(g_el);
                            }
                        }
                        col = col.child(sec);
                    }

                    // 3. GitHub Direct Notifications
                    if !inbox.github_stacks.is_empty() {
                        let mut sec = view().w_full().pt_3().gap_2();
                        for stack in inbox.github_stacks.clone() {
                            sec = sec.child(render_github_stack(ui, state, stack, now_ms));
                        }
                        col = col.child(sec);
                    }

                    // 4. De-emphasized collapsible summaries
                    if has_secondary {
                        let mut summaries: Vec<AnyElement> = Vec::new();
                        if sched_count > 0 {
                            let pulse = inbox.schedule_pulse.clone();
                            let upcoming = inbox.upcoming_schedules.clone();
                            summaries.push(
                                render_collapsible_summary(
                                    "Scheduled Runs".into(),
                                    sched_count,
                                    schedules_open,
                                    move || {
                                        render_schedule_rows(
                                            ui,
                                            state,
                                            pulse.clone(),
                                            upcoming.clone(),
                                        )
                                    },
                                )
                                .into_element(),
                            );
                        }
                        if shipped_count > 0 {
                            let shipped = inbox.shipped_today.clone();
                            summaries.push(
                                render_collapsible_summary(
                                    "Shipped Today".into(),
                                    shipped_count,
                                    shipped_open,
                                    move || render_shipped_rows(ui, state, shipped.clone(), now_ms),
                                )
                                .into_element(),
                            );
                        }
                        if watching_count > 0 {
                            let watching = inbox.github_watching.clone();
                            summaries.push(
                                render_collapsible_summary(
                                    "Watching".into(),
                                    watching_count,
                                    watching_open,
                                    move || {
                                        let rows: Vec<AnyElement> = watching
                                            .iter()
                                            .flat_map(|s| s.items.iter().cloned())
                                            .map(|it| {
                                                render_github_row_item(
                                                    ui, state, it, None, now_ms, false,
                                                )
                                            })
                                            .collect();
                                        hairline_list_inset(rows, 44.0).into_element()
                                    },
                                )
                                .into_element(),
                            );
                        }
                        if earlier_count > 0 {
                            let earlier_gh = inbox.earlier_github.clone();
                            let carryover = inbox.carryover_cards.clone();
                            summaries.push(
                                render_collapsible_summary(
                                    "Earlier".into(),
                                    earlier_count,
                                    earlier_open,
                                    move || {
                                        let mut rows: Vec<AnyElement> = Vec::new();
                                        for c in carryover.clone() {
                                            rows.push(render_open_card_row(ui, state, c, now_ms));
                                        }
                                        for gh in earlier_gh.clone() {
                                            rows.push(render_github_row_item(
                                                ui, state, gh, None, now_ms, false,
                                            ));
                                        }
                                        hairline_list_inset(rows, 44.0).into_element()
                                    },
                                )
                                .into_element(),
                            );
                        }
                        col = col
                            .child(view().w_full().pt_4().child(hairline_divider()))
                            .child(hairline_list(summaries));
                    }

                    col.into_element()
                })),
        ),
        scroll_ref,
        move || ui.refreshing.get(),
        move || {
            crate::hosts::reconnect_host(ui, None);
            refresh(ui);
            refresh_github_notifications(ui, true);
        },
    );

    view()
        .size_full()
        .flex_col()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(body)
}

/// Pushed native GitHub PR / Issue Markdown reader page with CI summary and bottom reply composer.
pub(crate) fn github_item_page(
    ui: Ui,
    state: MobileState,
    item: GitHubStackItem,
) -> impl IntoElement {
    load_github_item_detail(ui, &item);

    let key_lookup = item.key.clone();
    let repo_short = item
        .repository
        .rsplit('/')
        .next()
        .unwrap_or(&item.repository)
        .to_owned();
    let page_title = match item.number {
        Some(n) => format!("{repo_short} #{n}"),
        None => repo_short.clone(),
    };
    let url_open = item.url.clone();
    let item_done = item.clone();
    let key_defer = item.key.clone();
    let wm_defer = item.watermark;
    let title_defer = item.title.clone();

    let draft = signal(String::new());
    let sending = signal(false);

    let send_repo = item.repository.clone();
    let send_kind = item.kind.clone();
    let send_num = item.number;
    let send_url = item.url.clone();
    let send_comment = move || {
        let body = draft.get_untracked().trim().to_owned();
        if body.is_empty() || sending.get_untracked() {
            return;
        }
        trigger_haptic(Haptic::Selection);
        if !crate::backend::is_connected() {
            draft.set(String::new());
            ui.notify("Comment posted");
            return;
        }
        sending.set(true);
        let reference = GitHubReference {
            repository: send_repo.clone(),
            kind: send_kind.clone(),
            number: send_num,
            sha: None,
            url: send_url.clone(),
        };
        dispatch_complete(
            ui,
            "github.remote.comment",
            json!({ "reference": reference, "body": body }),
            move |res| {
                sending.try_set(false);
                if res.is_ok() {
                    draft.try_set(String::new());
                    ui.notify("Comment posted");
                }
            },
        );
    };

    let desktop_header = {
        let u = url_open.clone();
        let title_str = page_title.clone();
        view()
            .w_full()
            .h(px(52.))
            .px_3()
            .border_b_1()
            .border_color(BORDER)
            .bg(SURFACE)
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
                    .child(icon_tinted(Icon::Back, ACCENT).w(px(18.)).h(px(18.)))
                    .child(
                        text("Inbox")
                            .text_size(px(15.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ACCENT),
                    )
                    .on_press(move |_: &()| state.pop()),
            )
            .child(mobile_title(title_str).truncate().flex_1().min_w(px(0.)))
            .children((!u.is_empty()).then(move || {
                press_target()
                    .h(px(34.))
                    .px_2p5()
                    .rounded(px(8.))
                    .items_center()
                    .justify_center()
                    .child(
                        text("GitHub ↗")
                            .text_size(px(13.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ACCENT),
                    )
                    .on_press(move |_: &()| {
                        let target = u.clone();
                        ui.spawn(async move {
                            let _ = open_url(&target).await;
                        });
                    })
            }))
    };

    let mut page = nav_page()
        .key(format!("mobile-gh:{}", item.key))
        .title(page_title)
        .header_large_title(false)
        .header_shadow_visible(false)
        .header_style_with(|s| s.text_color(TEXT).tint_color(ACCENT))
        .hides_bottom_bar_when_pushed(true)
        .on_dismiss(move |_: &()| state.pop());

    if is_ios() {
        let u_ios = url_open.clone();
        let done_ios = item_done.clone();
        let k_def = key_defer.clone();
        let t_def = title_defer.clone();
        page =
            page.header_right_items(move || {
                let u = u_ios.clone();
                let d = done_ios.clone();
                let kd = k_def.clone();
                let td = t_def.clone();
                let mut items = Vec::new();
                if !u.is_empty() {
                    items.push(MenuItem::new("Open on GitHub").symbol("safari").on_select(
                        move || {
                            let target = u.clone();
                            ui.spawn(async move {
                                let _ = open_url(&target).await;
                            });
                        },
                    ));
                }
                items.push(MenuItem::new("Defer…").symbol("clock").on_select(move || {
                    state.open_defer(vec![(kd.clone(), wm_defer)], td.clone());
                }));
                items.push(
                    MenuItem::new("Mark Done")
                        .symbol("checkmark.circle")
                        .on_select(move || {
                            dismiss_github_item(ui, d.clone());
                            state.pop();
                        }),
                );
                vec![
                    HeaderItem::symbol("ellipsis.circle")
                        .key("gh-more")
                        .label("Actions")
                        .menu(items),
                ]
            });
    } else if is_android() {
        let u_and = url_open.clone();
        let done_and = item_done.clone();
        let k_def = key_defer.clone();
        let t_def = title_defer.clone();
        let mut items = Vec::new();
        if !u_and.is_empty() {
            items.push(MenuButtonItem::new("github", "Open on GitHub"));
        }
        items.push(MenuButtonItem::new("defer", "Defer…"));
        items.push(MenuButtonItem::new("done", "Mark Done"));
        page = page.header_right(
            menu_button()
                .title("Actions")
                .items(items)
                .on_select(move |choice: &SharedString| {
                    trigger_haptic(Haptic::Selection);
                    match choice.as_ref() {
                        "github" => {
                            let target = u_and.clone();
                            ui.spawn(async move {
                                let _ = open_url(&target).await;
                            });
                        }
                        "defer" => {
                            state.open_defer(vec![(k_def.clone(), wm_defer)], t_def.clone());
                        }
                        "done" => {
                            dismiss_github_item(ui, done_and.clone());
                            state.pop();
                        }
                        _ => {}
                    }
                })
                .child(
                    view()
                        .w(px(44.))
                        .h(px(44.))
                        .items_center()
                        .justify_center()
                        .child(icon_tinted(Icon::More, ACCENT).w(px(21.)).h(px(21.))),
                ),
        );
    } else {
        page = page.header(desktop_header);
    }

    let static_item = item.clone();
    page.child(
        view()
            .size_full()
            .flex_col()
            .min_h(px(0.))
            .bg(BACKGROUND)
            .child(
                scroll()
                    .flex_1()
                    .min_h(px(0.))
                    .child(view().w_full().p_4().gap_4().child(dynamic(move || {
                        let detail = ui
                            .inbox
                            .github_item_details
                            .with(|m| m.get(&key_lookup).cloned());
                        let loading = ui
                            .inbox
                            .github_item_loading
                            .with(|s| s.contains(&key_lookup));

                        let author = detail
                            .as_ref()
                            .and_then(|d| d.author_login.clone())
                            .or_else(|| static_item.author.clone());
                        let avatar_url = detail.as_ref().and_then(|d| d.author_avatar_url.clone());
                        let merged = detail.as_ref().is_some_and(|d| d.merged)
                            || static_item.pr_state.as_deref() == Some("merged");
                        let draft_pr = detail.as_ref().is_some_and(|d| d.draft)
                            || static_item.pr_state.as_deref() == Some("draft");
                        let pr_state = detail
                            .as_ref()
                            .map(|d| d.state.as_str())
                            .or(static_item.pr_state.as_deref());
                        let ci_state = detail
                            .as_ref()
                            .and_then(|d| d.ci_state.clone())
                            .or_else(|| static_item.ci_state.clone());
                        let (ci_passed, ci_total) = match detail.as_ref().filter(|d| d.ci_total > 0)
                        {
                            Some(d) => (d.ci_passed, d.ci_total),
                            None => (static_item.ci_passed, static_item.ci_total),
                        };
                        let failed_checks: Vec<String> = detail
                            .as_ref()
                            .filter(|d| !d.failed_checks.is_empty())
                            .map(|d| d.failed_checks.clone())
                            .unwrap_or_else(|| static_item.failed_checks.clone());
                        let comments = detail
                            .as_ref()
                            .and_then(|d| d.comments_count)
                            .or(static_item.comments_count);
                        let body_md = detail
                            .as_ref()
                            .map(|d| d.body.clone())
                            .filter(|b| !b.trim().is_empty())
                            .or_else(|| static_item.body.clone())
                            .unwrap_or_default();

                        let mut meta = view()
                            .w_full()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(gh_state_mark(&static_item.kind, pr_state, merged, draft_pr));
                        if let Some(a) = &author {
                            meta = meta.child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_1p5()
                                    .child(github_avatar(a, avatar_url.as_deref(), 18.))
                                    .child(mobile_footnote(format!("@{a}")).text_color(TEXT)),
                            );
                        }
                        if let Some(ci) = ci_indicator(
                            ci_state.as_deref(),
                            ci_passed,
                            ci_total,
                            failed_checks.len(),
                        ) {
                            meta = meta.child(ci);
                        }
                        if let Some(c) = comments
                            && c > 0
                        {
                            meta = meta.child(comments_indicator(c));
                        }

                        let mut col = view()
                            .w_full()
                            .gap_3()
                            .child(mobile_title(static_item.title.clone()))
                            .child(meta);

                        if let Some(d) = &detail {
                            let mut git_line = view()
                                .w_full()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2();
                            let mut has_git = false;
                            if let (Some(h), Some(b)) = (&d.head_ref, &d.base_ref) {
                                has_git = true;
                                git_line = git_line.child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            icon_tinted(Icon::Branch, MUTED).w(px(12.)).h(px(12.)),
                                        )
                                        .child(
                                            mobile_footnote(format!("{h} → {b}"))
                                                .font_family(MONO_FONT),
                                        ),
                                );
                            }
                            if let Some(a) = d.additions {
                                has_git = true;
                                git_line = git_line.child(
                                    text(format!("+{a}"))
                                        .text_size(px(12.5))
                                        .font_family(MONO_FONT)
                                        .text_color(SUCCESS),
                                );
                            }
                            if let Some(del) = d.deletions {
                                has_git = true;
                                git_line = git_line.child(
                                    text(format!("-{del}"))
                                        .text_size(px(12.5))
                                        .font_family(MONO_FONT)
                                        .text_color(DANGER),
                                );
                            }
                            if has_git {
                                col = col.child(git_line);
                            }
                        }

                        if let Some(cid) = static_item.card_id.clone() {
                            let card_lbl = match static_item.card_number {
                                Some(n) => format!("Linked Card #{n}"),
                                None => "Linked Card".to_owned(),
                            };
                            col = col.child(
                                press_target()
                                    .w_full()
                                    .px_3p5()
                                    .py_2p5()
                                    .rounded(px(10.))
                                    .bg(SURFACE)
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .active(|s| s.opacity(0.75))
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                icon_tinted(Icon::Board, ACCENT)
                                                    .w(px(15.))
                                                    .h(px(15.)),
                                            )
                                            .child(
                                                text(card_lbl)
                                                    .text_size(px(14.5))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(ACCENT),
                                            ),
                                    )
                                    .child(disclosure_chevron())
                                    .on_press(move |_: &()| {
                                        trigger_haptic(Haptic::Selection);
                                        state.push_card(cid.clone(), CardSegment::Conversation);
                                    }),
                            );
                        }

                        if !failed_checks.is_empty() {
                            let mut pills = view().w_full().flex_row().flex_wrap().gap_1p5();
                            for chk in failed_checks {
                                pills = pills.child(status(format!("✗ {chk}"), Tone::Danger));
                            }
                            col = col.child(pills);
                        }

                        col = col.child(hairline_divider());

                        if loading && detail.is_none() && body_md.is_empty() {
                            col = col.child(
                                view()
                                    .py_4()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(loading_spinner(15., ACCENT))
                                    .child(mobile_subhead("Loading…")),
                            );
                        } else if !body_md.is_empty() {
                            let md = body_md.clone();
                            col = col.child(
                                view()
                                    .w_full()
                                    .min_w(px(0.))
                                    .child(crate::editor::reader(ui, move || md.clone())),
                            );
                        } else {
                            col = col.child(mobile_footnote("No description"));
                        }

                        col.into_element()
                    }))),
            )
            .children(item.number.is_some().then(|| {
                view()
                    .w_full()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(BORDER)
                    .bg(SURFACE)
                    .px_3p5()
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
                            .pl_3p5()
                            .pr_1p5()
                            .py_1p5()
                            .flex_row()
                            .items_end()
                            .gap_2()
                            .child(
                                nsh_base::input()
                                    .value(move || draft.get())
                                    .on_change(move |v: &str| draft.set(v.to_owned()))
                                    .multiline(true)
                                    .placeholder("Comment on GitHub…")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .min_h(px(34.))
                                    .max_h(px(100.))
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
                                    .accessibility_label(Some(SharedString::from("Send comment")))
                                    .child(
                                        icon_tinted(Icon::ArrowUp, ON_ACCENT)
                                            .w(px(17.))
                                            .h(px(17.))
                                            .opacity(
                                                move || {
                                                    if can_send() { 1.0_f32 } else { 0.45_f32 }
                                                },
                                            ),
                                    )
                                    .on_press(move |_: &()| send_cb())
                            }),
                    )
            })),
    )
}

/// Bottom sheet listing Defer presets (`Next cutoff`, `In 2 hours`, `Tonight 20:00`, `Tomorrow 09:00`, `Next Monday`).
pub(crate) fn defer_sheet_body(
    ui: Ui,
    state: MobileState,
    items: Vec<(String, Option<u64>)>,
    subject: String,
) -> impl IntoElement {
    let cutoff = untracked(|| cutoff_minutes_from_utility(&ui.utility.get_untracked()));
    let presets = defer_presets(canban_types::now(), local_tz_offset_secs(), cutoff);
    let rows: Vec<AnyElement> = presets
        .into_iter()
        .map(|preset| {
            let its = items.clone();
            let subj = subject.clone();
            let lbl = preset.label.clone();
            let until = preset.until_ms;
            let until_str = format_defer_until(until);
            press_target()
                .w_full()
                .min_h(px(52.))
                .px_4()
                .py_2p5()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .active(|s| s.opacity(0.72))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2p5()
                        .child(icon_tinted(Icon::Clock, ATTENTION).w(px(16.)).h(px(16.)))
                        .child(
                            text(preset.label)
                                .text_size(px(MOBILE_BODY))
                                .text_color(TEXT),
                        ),
                )
                .child(mobile_footnote(until_str).tabular_nums())
                .on_press(move |_: &()| {
                    trigger_haptic(Haptic::Selection);
                    apply_defer(ui, its.clone(), until, &subj, &lbl);
                    state.close_sheet();
                })
                .into_element()
        })
        .collect();

    view()
        .w_full()
        .gap_3()
        .child(
            view().px_1().child(
                mobile_body(subject)
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate(),
            ),
        )
        .child(hairline_list(rows))
}
