//! Global Activity & Notifications popover (⌘I) + shared Overview rows.
use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_notifications::{OverviewRow, overview};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

pub fn toggle(ui: Ui) {
    crate::utilities::open(ui, "notifications");
}

pub fn close(ui: Ui) {
    if ui.utility_popup.get_untracked() == "notifications" {
        crate::utilities::close(ui);
    }
}

pub fn mark_card_read(ui: Ui, card_id: String, sequence: u64) {
    if crate::APP.get().is_none() {
        ui.snapshot.update(|s| {
            if let Some(existing) = s.card_reads.iter_mut().find(|r| r.card_id == card_id) {
                existing.sequence = existing.sequence.max(sequence);
            } else {
                let pid = s
                    .cards
                    .iter()
                    .find(|c| c.id == card_id)
                    .map(|c| c.project_id.clone())
                    .unwrap_or_default();
                s.card_reads.push(canban_types::CardRead {
                    id: card_id.clone(),
                    project_id: pid,
                    card_id,
                    sequence,
                });
            }
        });
        return;
    }
    dispatch(
        ui,
        "notifications.read",
        json!({"card_id": card_id, "sequence": sequence}),
    );
}

pub fn mark_all_read(ui: Ui, project_scope: Option<String>) {
    if crate::APP.get().is_none() {
        ui.snapshot.update(|s| {
            let cards: Vec<_> = s
                .cards
                .iter()
                .filter(|c| {
                    c.removed_at.is_none()
                        && project_scope
                            .as_deref()
                            .is_none_or(|pid| c.project_id == pid)
                })
                .map(|c| (c.id.clone(), c.project_id.clone()))
                .collect();
            for (cid, pid) in cards {
                let max_seq = s
                    .events
                    .iter()
                    .filter(|e| e.card_id == cid)
                    .map(|e| e.sequence)
                    .max()
                    .unwrap_or(0);
                if max_seq == 0 {
                    continue;
                }
                if let Some(existing) = s.card_reads.iter_mut().find(|r| r.card_id == cid) {
                    existing.sequence = existing.sequence.max(max_seq);
                } else {
                    s.card_reads.push(canban_types::CardRead {
                        id: cid.clone(),
                        project_id: pid,
                        card_id: cid,
                        sequence: max_seq,
                    });
                }
            }
        });
        ui.notify("Marked updates read");
        return;
    }
    dispatch_then(
        ui,
        "notifications.read_all",
        json!({"project_id": project_scope}),
        move |_| ui.notify("Marked updates read"),
    );
}

fn is_special_agent_terminal(s: &canban_types::Snapshot, e: &crate::terminals::Entry) -> bool {
    e.agent.is_none()
        && (s.configuration_sessions.iter().any(|c| c.id == e.saved.id)
            || e.saved.agent_engine.is_some())
}

fn attention_summary(ui: Ui) -> (usize, usize, usize, usize) {
    let s = ui.snapshot.get();
    let counts = ui.reminder_counts.get();
    let all_rows = overview(&s);
    let actionable_cards: std::collections::BTreeSet<_> = all_rows
        .iter()
        .filter(|r| r.actionable())
        .map(|r| r.card.id.clone())
        .collect();
    let actionable = all_rows.iter().filter(|r| r.actionable()).count()
        + crate::terminals::entries(&s)
            .into_iter()
            .filter(|e| {
                e.listed()
                    && (e.needs_attention() || e.status() == "blocked")
                    && is_special_agent_terminal(&s, e)
            })
            .count();
    let unread_cards = s
        .cards
        .iter()
        .filter(|c| {
            counts.get(&c.id).is_some_and(|n| n.unread > 0) && !actionable_cards.contains(&c.id)
        })
        .count();
    let active_slots = canban_scheduler::active_count(&s);
    let max_slots = ui.max_agents.get();
    (actionable, unread_cards, active_slots, max_slots)
}

pub fn trigger_pill(ui: Ui) -> impl IntoElement {
    let stats = computed(move || attention_summary(ui));
    let is_open = computed(move || ui.utility_popup.get() == "notifications");
    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(
            hint_tooltip(move || {
                let (actionable, unread, active, max) = stats.get();
                format!(
                    "Activity & Notifications · ⌘I\n{} needs you · {} unread · {} / {} running",
                    actionable, unread, active, max
                )
            })
            .placement(Placement::Bottom),
        )
        .child(dynamic(move || {
            let (actionable, unread, active, max) = stats.get();
            let total_alerts = actionable + unread;
            let open = is_open.get();
            let has_urgent = actionable > 0;
            let border = if open {
                ACCENT
            } else if has_urgent {
                ATTENTION
            } else {
                BORDER
            };
            let bg = if open {
                SELECTED
            } else if has_urgent {
                ATTENTION_SURFACE
            } else {
                INSET
            };
            let mut btn = canban_ui_kit::press_target()
                .h(px(28.))
                .px_2p5()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(border)
                .bg(bg)
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                .flex_row()
                .items_center()
                .gap_1p5()
                .accessibility_label(move || {
                    Some(
                        format!(
                            "Activity & Notifications, {} alerts, {} of {} running",
                            total_alerts, active, max
                        )
                        .into(),
                    )
                })
                .on_press(move |_: &()| toggle(ui));
            btn = btn.child(
                icon_tinted(
                    Icon::Overview,
                    if has_urgent {
                        ATTENTION
                    } else if unread > 0 || open {
                        ACCENT
                    } else {
                        MUTED
                    },
                )
                .w(px(13.5))
                .h(px(13.5)),
            );
            if total_alerts > 0 {
                btn = btn
                    .child(
                        label(total_alerts.to_string())
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if has_urgent { ATTENTION } else { TEXT }),
                    )
                    .child(muted("·").text_size(px(11.)));
            }
            btn = btn
                .child(view().w(px(6.)).h(px(6.)).rounded_full().bg(if active > 0 {
                    SUCCESS
                } else {
                    MUTED
                }))
                .child(
                    muted(format!("{active}/{max}"))
                        .text_size(px(11.5))
                        .font_family("Menlo"),
                );
            btn
        }))
}

pub fn popover(ui: Ui) -> impl IntoElement {
    let project_scope = signal(String::new());
    nsh_base::dialog()
        .animation(canban_ui_kit::motion::modal)
        .close_on_escape(true)
        .close_on_backdrop_press(true)
        .on_cancel(move || close(ui))
        .child(
            view()
                .flex_1()
                .items_end()
                .justify_start()
                .pt(px(46.))
                .pr(px(12.))
                .pb(px(36.))
                .pl(px(12.))
                .bg(rgba(0x00000022))
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .label("Dismiss Activity & Notifications")
                        .on_press(move || close(ui)),
                )
                .child(
                    panel()
                        .w(px(480.))
                        .max_w(Percentage(0.95))
                        .p_0()
                        .gap_0()
                        .overflow_hidden()
                        .border_1()
                        .border_color(BORDER)
                        .bg(SURFACE)
                        .child(
                            view()
                                .flex_shrink_0()
                                .px_3p5()
                                .py_2p5()
                                .border_b_1()
                                .border_color(BORDER)
                                .gap_2()
                                .child(dynamic(move || {
                                    let s = ui.snapshot.get();
                                    let counts = ui.reminder_counts.get();
                                    let active_slots = canban_scheduler::active_count(&s);
                                    let max_slots = ui.max_agents.get();
                                    let queued_count =
                                        s.attempts.iter().filter(|a| a.state == "queued").count();
                                    let scope = project_scope.get();
                                    let valid_scope = if !scope.is_empty()
                                        && s.projects.iter().any(|p| p.id == scope)
                                    {
                                        Some(scope)
                                    } else {
                                        None
                                    };
                                    let has_unread = s.cards.iter().any(|c| {
                                        valid_scope.as_deref().is_none_or(|pid| c.project_id == pid)
                                            && counts.get(&c.id).is_some_and(|n| n.unread > 0)
                                    });
                                    let scope_for_read = valid_scope.clone();
                                    let mut top = view()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(
                                            view()
                                                .flex_row()
                                                .items_center()
                                                .gap_2()
                                                .min_w(px(0.))
                                                .child(
                                                    label("Activity & Notifications")
                                                        .font_weight(FontWeight::SEMIBOLD),
                                                )
                                                .child(status(
                                                    format!("{active_slots}/{max_slots} running"),
                                                    if active_slots > 0 {
                                                        Tone::Info
                                                    } else {
                                                        Tone::Neutral
                                                    },
                                                ))
                                                .child(when(
                                                    move || queued_count > 0,
                                                    move || badge(format!("{queued_count} queued")),
                                                )),
                                        );
                                    let mut right = view().flex_row().items_center().gap_1();
                                    if has_unread {
                                        right = right.child(compact_action(
                                            "✓ Read all",
                                            Variant::Quiet,
                                            move |_| mark_all_read(ui, scope_for_read.clone()),
                                        ));
                                    }
                                    right = right.child(chrome_button(
                                        Icon::Close,
                                        "Close Activity & Notifications",
                                        move |_| close(ui),
                                    ));
                                    top = top.child(right);

                                    if s.projects.len() > 1 {
                                        let all_selected = valid_scope.is_none();
                                        let mut filter_seg = segmented().child(compact_segment(
                                            "All",
                                            None,
                                            move || all_selected,
                                            move |_| project_scope.set(String::new()),
                                        ));
                                        for p in &s.projects {
                                            let pid = p.id.clone();
                                            let is_selected =
                                                valid_scope.as_deref() == Some(pid.as_str());
                                            filter_seg = filter_seg.child(compact_segment(
                                                p.name.clone(),
                                                None,
                                                move || is_selected,
                                                move |_| project_scope.set(pid.clone()),
                                            ));
                                        }
                                        view()
                                            .gap_2()
                                            .child(top)
                                            .child(
                                                scroll()
                                                    .horizontal()
                                                    .scrollbar(false)
                                                    .child(filter_seg),
                                            )
                                            .into_element()
                                    } else {
                                        top.into_element()
                                    }
                                })),
                        )
                        .child(scroll().max_h(px(560.)).p_3p5().child(dynamic(move || {
                            activity_body(ui, project_scope.get(), stored(true), true)
                        }))),
                ),
        )
}

fn destination(ui: Ui, row: &OverviewRow, review: bool) {
    close(ui);
    ui.open_card(row.card.id.clone());
    if let Some(id) = row.outcome_attempt_id() {
        ui.request.set(id.into());
    }
    if review {
        ui.page.set("review".into());
    }
}

pub fn show_card_attention(ui: Ui, card: &str) {
    let rows: Vec<_> = overview(&ui.snapshot.get_untracked())
        .into_iter()
        .filter(|r| r.card.id == card && r.actionable())
        .collect();
    if rows.len() == 1 && rows[0].reasons.len() == 1 {
        activate(ui, &rows[0]);
        return;
    }
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    let mut items = Vec::new();
    for row in rows {
        for reason in &row.reasons {
            let saved = row.clone();
            let source = reason.source.clone();
            items.push(MenuItem::new(reason.message.clone()).on_select(move || {
                if source == "runtime" {
                    close(ui);
                    ui.open_card_with_terminal(saved.card.id.clone(), Some(saved.id.clone()));
                } else {
                    destination(ui, &saved, source == "review");
                }
            }));
        }
    }
    if items.is_empty() {
        ui.open_card(card.into());
    } else {
        ui.spawn(async move {
            popup_menu(items).await;
        });
    }
}

fn activate(ui: Ui, row: &OverviewRow) {
    ui.overview_selection.set(row.id.clone());
    close(ui);
    if row.reasons.iter().any(|r| r.source == "review") {
        destination(ui, row, true);
    } else if row.reasons.iter().any(|r| r.source != "runtime") {
        destination(ui, row, false);
    } else if crate::terminals::model::binding(row).is_some() {
        ui.open_card_with_terminal(row.card.id.clone(), Some(row.id.clone()));
    } else {
        crate::terminals::navigation::history(ui, row);
    }
}

fn primary(row: &OverviewRow) -> &'static str {
    if row.reasons.iter().any(|r| r.source == "review") {
        "Review →"
    } else if row.reasons.iter().any(|r| r.source != "runtime") {
        "Open Card →"
    } else if crate::terminals::model::binding(row).is_some() {
        "Card & Terminal →"
    } else if crate::terminals::model::closed(row) {
        "View history →"
    } else {
        "Open Card →"
    }
}

fn summary(row: &OverviewRow) -> String {
    if !row.reasons.is_empty() {
        return row
            .reasons
            .iter()
            .map(|r| r.message.clone())
            .collect::<Vec<_>>()
            .join(" · ");
    }
    if let Some(a) = &row.attempt {
        if a.finished() {
            return "Agent ended · saved history available".into();
        }
        if a.connection_state != "connected" {
            return "Connection unavailable · original Terminal retained".into();
        }
        if let Some(v) = &a.observed
            && let Some(message) = v["status"]["msg"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| {
                    v["status"]["title"]
                        .as_str()
                        .filter(|s| !s.trim().is_empty())
                })
        {
            return format!("Agent: {}", message.lines().collect::<Vec<_>>().join(" "));
        }
    }
    match row.group.as_str() {
        "Queued" => "Waiting for execution admission",
        "Working" => "Agent working",
        "Idle" => "Agent idle",
        "Closed" => "Saved conversation",
        _ => "View saved request",
    }
    .into()
}

fn row_visual(row: &OverviewRow) -> (Icon, ThemeColor) {
    let profile_icon = row
        .attempt
        .as_ref()
        .map(|a| crate::profiles::provider(&a.profile).0);
    if row.reasons.iter().any(|r| r.source == "review") {
        (Icon::Check, ATTENTION)
    } else if row.reasons.iter().any(|r| r.source == "recovery") {
        (Icon::Info, DANGER)
    } else if row.reasons.iter().any(|r| r.source == "runtime") {
        (Icon::Terminal, ATTENTION)
    } else if row.actionable() {
        (Icon::Info, ATTENTION)
    } else {
        match row.group.as_str() {
            "Working" => (profile_icon.unwrap_or(Icon::Agent), ACCENT),
            "Queued" => (Icon::History, MUTED),
            "Idle" => (profile_icon.unwrap_or(Icon::Agent), MUTED),
            _ => (Icon::History, MUTED),
        }
    }
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let focus_restored = stored(false);
    let project_scope = signal(String::new());
    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(dynamic(move || {
            let s = ui.snapshot.get();
            let all_rows = overview(&s);
            let actionable_count = all_rows.iter().filter(|r| r.actionable()).count()
                + crate::terminals::entries(&s)
                    .into_iter()
                    .filter(|e| {
                        e.listed()
                            && (e.needs_attention() || e.status() == "blocked")
                            && is_special_agent_terminal(&s, e)
                    })
                    .count();
            let active_slots = canban_scheduler::active_count(&s);
            let max_slots = ui.max_agents.get();
            let queued_count = s.attempts.iter().filter(|a| a.state == "queued").count();
            let scope = project_scope.get();
            let valid_scope = if !scope.is_empty() && !s.projects.iter().any(|p| p.id == scope) {
                String::new()
            } else {
                scope
            };

            let mut pills = view().flex_row().flex_wrap().items_center().gap_2();
            if actionable_count > 0 {
                pills = pills.child(status(
                    format!("{actionable_count} needs you"),
                    Tone::Attention,
                ));
            }
            pills = pills.child(status(
                format!("Running {active_slots} / {max_slots}"),
                if active_slots > 0 {
                    Tone::Info
                } else {
                    Tone::Neutral
                },
            ));
            if queued_count > 0 {
                pills = pills.child(badge(format!("Queued {queued_count}")));
            }

            let mut header = view()
                .max_w(px(1080.))
                .w_full()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(heading("Overview"))
                        .child(pills),
                );

            if s.projects.len() > 1 {
                let all_selected = valid_scope.is_empty();
                let mut filter_seg = segmented().child(compact_segment(
                    "All projects",
                    None,
                    move || all_selected,
                    move |_| project_scope.set(String::new()),
                ));
                for p in &s.projects {
                    let pid = p.id.clone();
                    let is_selected = valid_scope == pid;
                    filter_seg = filter_seg.child(compact_segment(
                        p.name.clone(),
                        None,
                        move || is_selected,
                        move |_| project_scope.set(pid.clone()),
                    ));
                }
                header = header.child(filter_seg);
            }

            header.into_element()
        }))
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .content_offset(scroll_position(ui, "overview:list"))
                .on_scroll(move |e: &ScrollEvent| capture_scroll(ui, "overview:list".into(), e))
                .child(dynamic(move || {
                    activity_body(ui, project_scope.get(), focus_restored, false)
                })),
        )
}

fn activity_body(
    ui: Ui,
    raw_scope: String,
    focus_restored: Stored<bool>,
    compact_popover: bool,
) -> View {
    let s = ui.snapshot.get();
    let scope = if !raw_scope.is_empty() && s.projects.iter().any(|p| p.id == raw_scope) {
        Some(raw_scope)
    } else {
        None
    };
    let mut rows: Vec<_> = overview(&s)
        .into_iter()
        .filter(|r| scope.as_deref().is_none_or(|pid| r.card.project_id == pid))
        .collect();
    // Stable task/profile order rather than continuously changing activity timestamps.
    rows.sort_by(|a, b| {
        (&a.project_name, a.card.number, &a.id).cmp(&(&a.project_name, b.card.number, &b.id))
    });

    let mut content = view()
        .max_w(px(if compact_popover { 10000. } else { 1080. }))
        .w_full()
        .gap(px(if compact_popover { 16. } else { 24. }))
        .pb_2()
        .min_w(px(0.));
    let mut updates = unread_updates(ui, scope.as_deref());
    let mut any_rendered = false;

    for group in ["Needs you", "Running", "Queued", "Idle", "Closed"] {
        if group == "Running"
            && let Some(v) = updates.take()
        {
            any_rendered = true;
            content = content.child(v);
        }
        let members: Vec<_> = rows
            .iter()
            .filter(|r| match group {
                "Needs you" => r.actionable(),
                "Running" => r.group == "Working" && !r.actionable(),
                other => r.group == other && !r.actionable(),
            })
            .cloned()
            .collect();
        let configurations: Vec<_> = if matches!(group, "Running" | "Needs you") {
            crate::terminals::entries(&s)
                .into_iter()
                .filter(|e| {
                    e.listed()
                        && scope
                            .as_deref()
                            .is_none_or(|pid| e.project_id.as_deref() == Some(pid))
                        && if group == "Needs you" {
                            (e.needs_attention() || e.status() == "blocked")
                                && is_special_agent_terminal(&s, e)
                        } else {
                            !(e.needs_attention() || e.status() == "blocked")
                                && s.configuration_sessions.iter().any(|c| c.id == e.saved.id)
                        }
                })
                .collect()
        } else {
            vec![]
        };
        let count = members.len() + configurations.len();
        if count == 0 {
            continue;
        }
        any_rendered = true;

        let build_rows = |members: Vec<OverviewRow>,
                          configurations: Vec<crate::terminals::Entry>| {
            let mut stack = view().min_w(px(0.));
            for row in members {
                let selected = ui.overview_selection.get() == row.id;
                stack = stack.child(row_view(ui, row, selected, focus_restored));
            }
            for entry in configurations {
                stack = stack.child(config_session_row(ui, entry));
            }
            stack
        };

        if matches!(group, "Idle" | "Closed") {
            let is_open = if group == "Closed" {
                ui.overview_closed.get()
            } else {
                ui.overview_idle.get()
            };
            let title = if group == "Closed" {
                "Closed conversations"
            } else {
                "Idle"
            };
            let body = if is_open {
                build_rows(members, configurations).into_element()
            } else {
                view().into_element()
            };
            content = content.child(collapsible_section_group(
                title,
                Some(count),
                is_open,
                move |_| {
                    if group == "Closed" {
                        ui.overview_closed.update(|v| *v = !*v);
                    } else {
                        ui.overview_idle.update(|v| *v = !*v);
                    }
                },
                body,
            ));
        } else {
            let tone = match group {
                "Needs you" => Tone::Attention,
                "Running" => Tone::Info,
                _ => Tone::Neutral,
            };
            let mut sec = section_group(group).count(count).tone(tone);
            if group == "Running" {
                sec = sec.trailing(compact_action("Terminals ↗", Variant::Quiet, move |_| {
                    close(ui);
                    ui.navigate("terminals");
                }));
            }
            content = content.child(sec.child(build_rows(members, configurations)));
        }
    }

    if let Some(v) = updates.take() {
        any_rendered = true;
        content = content.child(v);
    }

    if !any_rendered {
        content = content.child(crate::components::activity::empty(
            "All clear",
            "No pending gates, unread updates, or active Agent runs.",
        ));
    }
    content
}

/// Production row reused by Gallery. Destination controls remain available without hover.
pub fn agent_row(ui: Ui, row: OverviewRow, selected: bool) -> impl IntoElement {
    row_view(ui, row, selected, stored(true))
}

fn config_session_row(ui: Ui, entry: crate::terminals::Entry) -> impl IntoElement {
    let id = entry.saved.id.clone();
    let id_btn = id.clone();
    let is_config = ui
        .snapshot
        .with_untracked(|s| s.configuration_sessions.iter().any(|c| c.id == id));
    let needs_attn = entry.needs_attention() || entry.status() == "blocked";
    let status_text = entry
        .attention_reason()
        .unwrap_or_else(|| entry.display_status());
    let project_name = entry.project_name.clone();
    let row_title = if is_config {
        "Edit Project Workflows".into()
    } else if let Some(c) = &entry.card {
        format!("#{} {} · {}", c.number, c.title, entry.title())
    } else {
        entry.title()
    };
    let subtitle = if is_config {
        format!("Workflow configuration session · {status_text}")
    } else {
        status_text
    };
    let row_icon = if is_config {
        Icon::Trigger
    } else {
        entry.provider().map(|(ic, _)| ic).unwrap_or(Icon::Agent)
    };
    let card_dest = entry.card.as_ref().map(|c| c.id.clone());
    let card_dest_btn = card_dest.clone();

    view()
        .flex_row()
        .items_center()
        .gap_3()
        .py_2p5()
        .px_2()
        .min_w(px(0.))
        .border_b_1()
        .border_color(BORDER)
        .child(
            canban_ui_kit::press_target()
                .flex_1()
                .min_w(px(0.))
                .items_start()
                .flex_row()
                .gap_2p5()
                .bg(CLEAR)
                .rounded(px(RADIUS))
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                .on_press(move |_: &()| {
                    close(ui);
                    if let Some(cid) = &card_dest {
                        ui.open_card_with_terminal(cid.clone(), Some(id.clone()));
                    } else {
                        ui.open_terminal(id.clone());
                    }
                })
                .child(
                    view().pt_0p5().child(
                        icon_tinted(row_icon, if needs_attn { ATTENTION } else { ACCENT })
                            .w(px(15.))
                            .h(px(15.)),
                    ),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_1()
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .min_w(px(0.))
                                .child(label(row_title).font_weight(FontWeight::MEDIUM).truncate())
                                .child(badge(project_name)),
                        )
                        .child(
                            muted(subtitle)
                                .text_color(if needs_attn { ATTENTION } else { MUTED })
                                .truncate()
                                .min_w(px(0.)),
                        ),
                ),
        )
        .child(compact_action(
            "Terminal →",
            Variant::Secondary,
            move |_| {
                close(ui);
                if let Some(cid) = &card_dest_btn {
                    ui.open_card_with_terminal(cid.clone(), Some(id_btn.clone()));
                } else {
                    ui.open_terminal(id_btn.clone());
                }
            },
        ))
}

fn row_view(
    ui: Ui,
    row: OverviewRow,
    selected: bool,
    focus_restored: Stored<bool>,
) -> impl IntoElement {
    let reference = element_ref();
    let activate_row = row.clone();
    let text = summary(&row);
    let (kind_icon, kind_color) = row_visual(&row);
    let profile_tag = row.attempt.as_ref().map(|a| format!("@{}", a.profile.id));
    let remote_machine = row
        .attempt
        .as_ref()
        .and_then(|a| a.location.as_ref())
        .map(|l| l.machine.as_str())
        .filter(|m| !m.is_empty() && *m != "local")
        .map(str::to_owned);
    let waiting_age = row
        .attempt
        .as_ref()
        .and_then(|a| a.lifecycle.waiting.as_ref().map(|w| w.since))
        .map(|since| {
            format!(
                "Waiting {}",
                crate::components::activity::age(since).trim_end_matches(" ago")
            )
        });
    let unread = ui
        .reminder_counts
        .with(|m| m.get(&row.card.id).map(|c| c.unread).unwrap_or(0));

    let metadata_str = match &profile_tag {
        Some(p) => format!("{p} · {}", row.project_name),
        None => row.project_name.clone(),
    };
    let hint = format!(
        "#{} {}\n{metadata_str}\n{text}",
        row.card.number, row.card.title
    );

    let mut meta_pills = view().flex_row().items_center().gap_1p5().flex_shrink_0();
    if let Some(p) = profile_tag.clone() {
        meta_pills = meta_pills.child(muted(p));
    }
    meta_pills = meta_pills.child(
        muted(row.project_name.clone())
            .max_w(px(if ui.narrow.get() { 110. } else { 200. }))
            .truncate(),
    );
    if let Some(machine) = remote_machine {
        meta_pills = meta_pills.child(badge(machine));
    }
    if let Some(wait) = waiting_age {
        meta_pills = meta_pills.child(status(wait, Tone::Attention));
    }
    if unread > 0 && row.actionable() {
        meta_pills = meta_pills.child(status(format!("{unread} unread"), Tone::Info));
    }

    let main = canban_ui_kit::press_target()
        .element_ref(&reference)
        .flex_1()
        .min_w(px(0.))
        .items_start()
        .flex_row()
        .gap_2p5()
        .py_2()
        .px_2()
        .rounded(px(RADIUS))
        .bg(CLEAR)
        .hover(|s| s.bg(CARD_HOVER))
        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
        .accessibility_label(format!(
            "#{} {} · {metadata_str} · {} · {}",
            row.card.number,
            row.card.title,
            text,
            primary(&row)
        ))
        .child(
            view()
                .pt_0p5()
                .child(icon_tinted(kind_icon, kind_color).w(px(15.)).h(px(15.))),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .gap_1()
                .child(
                    view()
                        .flex_row()
                        .w_full()
                        .items_center()
                        .gap_2()
                        .min_w(px(0.))
                        .child(muted(format!("#{}", row.card.number)).flex_shrink_0())
                        .child(
                            label(row.card.title.clone())
                                .font_weight(FontWeight::MEDIUM)
                                .flex_1()
                                .min_w(px(0.))
                                .truncate(),
                        )
                        .child(meta_pills),
                )
                .child(
                    muted(text)
                        .text_color(if row.actionable() { ATTENTION } else { MUTED })
                        .truncate()
                        .w_full()
                        .min_w(px(0.)),
                ),
        );

    let button_row = row.clone();
    let mut actions = view().flex_row().items_center().gap_1p5().flex_shrink_0();

    if row.reasons.iter().any(|r| r.source == "review") {
        let project = ui.snapshot.with_untracked(|s| {
            s.projects
                .iter()
                .find(|p| p.id == row.card.project_id)
                .cloned()
        });
        if crate::card::quick_merge_eligible(&row.card, project.as_ref()) {
            let merge_card = row.card.clone();
            actions = actions.child(
                compact_action("Merge", Variant::Quiet, move |_| {
                    close(ui);
                    crate::card::quick_merge_start(ui, merge_card.clone());
                })
                .disabled(move || crate::card::quick_merge_busy(ui)),
            );
        }
    }
    if crate::terminals::model::can_resume(&row, &ui.profiles.get()) {
        let id = row.id.clone();
        actions = actions.child(
            compact_action("Resume Agent", Variant::Quiet, move |_| {
                ui.overview_selection.set(id.clone());
                close(ui);
                crate::terminals::navigation::resume(ui, &id)
            })
            .disabled(move || ui.pending.get()),
        );
    }
    if row.reasons.iter().any(|r| r.source != "runtime")
        && crate::terminals::model::binding(&row).is_some()
    {
        let id = row.id.clone();
        let cid = row.card.id.clone();
        actions = actions.child(compact_action(
            "Card & Terminal",
            Variant::Quiet,
            move |_| {
                ui.overview_selection.set(id.clone());
                close(ui);
                ui.open_card_with_terminal(cid.clone(), Some(id.clone()));
            },
        ));
    }
    let primary_variant = if row.actionable() {
        Variant::Secondary
    } else {
        Variant::Quiet
    };
    actions = actions.child(compact_action(primary(&row), primary_variant, move |_| {
        activate(ui, &button_row)
    }));

    view()
        .flex_row()
        .items_center()
        .gap_2()
        .py_0p5()
        .min_w(px(0.))
        .border_b_1()
        .border_color(BORDER)
        .rounded(px(4.))
        .bg(if selected { SELECTED } else { CLEAR })
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .on_layout(move |_: &Bounds<Points>| {
                    if selected && !focus_restored.replace(true) {
                        reference.focus();
                    }
                })
                .tooltip(hint_tooltip(hint))
                .child(main.on_press(move |_: &()| activate(ui, &activate_row))),
        )
        .child(actions)
}

fn unread_updates(ui: Ui, scope: Option<&str>) -> Option<SectionGroup> {
    let counts = ui.reminder_counts.get();
    let s = ui.snapshot.get();
    let items = ui.reminders.get();
    let actionable: std::collections::BTreeSet<_> = overview(&s)
        .iter()
        .filter(|r| r.actionable())
        .map(|r| r.card.id.clone())
        .collect();
    let cards: Vec<_> = s
        .cards
        .iter()
        .filter(|c| {
            scope.is_none_or(|pid| c.project_id == pid)
                && counts.get(&c.id).is_some_and(|n| n.unread > 0)
                && !actionable.contains(&c.id)
        })
        .collect();
    if cards.is_empty() {
        return None;
    }

    let mut rows = view().min_w(px(0.));
    for card in &cards {
        let id = card.id.clone();
        let id_btn = id.clone();
        let id_read = id.clone();
        let latest = items
            .iter()
            .filter(|i| i.card_id == id && i.unread)
            .max_by_key(|i| i.sequence);
        let message = latest
            .map(|i| i.message.lines().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let sequence = latest.map(|i| i.sequence);
        let max_seq = s
            .events
            .iter()
            .filter(|e| e.card_id == id)
            .map(|e| e.sequence)
            .max()
            .unwrap_or(0);
        let unread_count = counts[&id].unread;
        let project = s
            .projects
            .iter()
            .find(|p| p.id == card.project_id)
            .map(|p| p.name.as_str())
            .unwrap_or("")
            .to_owned();

        rows = rows.child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .py_0p5()
                .min_w(px(0.))
                .border_b_1()
                .border_color(BORDER)
                .child(
                    canban_ui_kit::press_target()
                        .flex_1()
                        .min_w(px(0.))
                        .items_start()
                        .flex_row()
                        .gap_2p5()
                        .py_2()
                        .px_2()
                        .rounded(px(RADIUS))
                        .bg(CLEAR)
                        .hover(|s| s.bg(CARD_HOVER))
                        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                        .on_press(move |_: &()| {
                            close(ui);
                            ui.open_card(id.clone());
                            ui.card_event.set(sequence);
                        })
                        .child(
                            view()
                                .pt_1p5()
                                .child(view().w(px(8.)).h(px(8.)).rounded_full().bg(ACCENT)),
                        )
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.))
                                .gap_1()
                                .child(
                                    view()
                                        .flex_row()
                                        .w_full()
                                        .items_center()
                                        .gap_2()
                                        .min_w(px(0.))
                                        .child(muted(format!("#{}", card.number)).flex_shrink_0())
                                        .child(
                                            label(card.title.clone())
                                                .font_weight(FontWeight::MEDIUM)
                                                .flex_1()
                                                .min_w(px(0.))
                                                .truncate(),
                                        )
                                        .child(
                                            muted(project)
                                                .max_w(px(140.))
                                                .truncate()
                                                .flex_shrink_0(),
                                        )
                                        .child(status(
                                            format!("{unread_count} unread"),
                                            Tone::Info,
                                        )),
                                )
                                .child(muted(message).truncate().w_full().min_w(px(0.))),
                        ),
                )
                .child(compact_action("✓ Read", Variant::Quiet, move |_| {
                    mark_card_read(ui, id_read.clone(), max_seq);
                }))
                .child(compact_action("Card →", Variant::Secondary, move |_| {
                    close(ui);
                    ui.open_card(id_btn.clone());
                    ui.card_event.set(sequence);
                })),
        );
    }

    Some(
        section_group("Updates")
            .count(cards.len())
            .tone(Tone::Info)
            .child(rows),
    )
}
