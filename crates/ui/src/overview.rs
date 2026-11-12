//! Global handling and execution, with direct destinations and no intermediate inspector.
use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_notifications::{OverviewRow, overview};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;

fn destination(ui: Ui, row: &OverviewRow, review: bool) {
    ui.open_card(row.card.id.clone());
    if let Some(id) = row.outcome_request_id() {
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
                    ui.open_terminal(saved.id.clone());
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
    if row.reasons.iter().any(|r| r.source == "review") {
        destination(ui, row, true);
    } else if row.reasons.iter().any(|r| r.source != "runtime") {
        destination(ui, row, false);
    } else if crate::terminals::model::binding(row).is_some() {
        ui.open_terminal(row.id.clone());
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
        "Terminal →"
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
        .request
        .as_ref()
        .map(|r| crate::profiles::provider(&r.profile).0);
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
                            && s.configuration_sessions.iter().any(|c| c.id == e.saved.id)
                    })
                    .count();
            let active_slots = canban_scheduler::active_count(&s);
            let max_slots = ui.max_agents.get();
            let queued_count = s.requests.iter().filter(|r| r.state == "queued").count();
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
                    let s = ui.snapshot.get();
                    let raw_scope = project_scope.get();
                    let scope =
                        if !raw_scope.is_empty() && s.projects.iter().any(|p| p.id == raw_scope) {
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
                        (&a.project_name, a.card.number, &a.id).cmp(&(
                            &b.project_name,
                            b.card.number,
                            &b.id,
                        ))
                    });

                    let mut content = view()
                        .max_w(px(1080.))
                        .w_full()
                        .gap_6()
                        .pb_4()
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
                                        && (group == "Needs you")
                                            == (e.needs_attention() || e.status() == "blocked")
                                        && s.configuration_sessions
                                            .iter()
                                            .any(|c| c.id == e.saved.id)
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
                            content = content.child(
                                section_group(group)
                                    .count(count)
                                    .tone(tone)
                                    .child(build_rows(members, configurations)),
                            );
                        }
                    }

                    if let Some(v) = updates.take() {
                        any_rendered = true;
                        content = content.child(v);
                    }

                    if !any_rendered {
                        content = content.child(crate::components::activity::empty(
                            "No Agent activity",
                            "Start work from a Card on the Board or open Project Workflows.",
                        ));
                    }
                    content
                })),
        )
}

/// Production row reused by Gallery. Destination controls remain available without hover.
pub fn agent_row(ui: Ui, row: OverviewRow, selected: bool) -> impl IntoElement {
    row_view(ui, row, selected, stored(true))
}

fn config_session_row(ui: Ui, entry: crate::terminals::Entry) -> impl IntoElement {
    let id = entry.saved.id.clone();
    let id_btn = id.clone();
    let needs_attn = entry.needs_attention() || entry.status() == "blocked";
    let status_text = entry.display_status();
    let project_name = entry.project_name.clone();

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
            nsh_base::button()
                .flex_1()
                .min_w(px(0.))
                .items_start()
                .flex_row()
                .gap_2p5()
                .bg(CLEAR)
                .rounded(px(RADIUS))
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                .on_press(move |_: &()| ui.open_terminal(id.clone()))
                .child(
                    view().pt_0p5().child(
                        icon_tinted(Icon::Trigger, if needs_attn { ATTENTION } else { ACCENT })
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
                                .child(
                                    label("Edit Project Workflows")
                                        .font_weight(FontWeight::MEDIUM)
                                        .truncate(),
                                )
                                .child(badge(project_name)),
                        )
                        .child(
                            muted(format!("Workflow configuration session · {status_text}"))
                                .text_color(if needs_attn { ATTENTION } else { MUTED })
                                .truncate()
                                .min_w(px(0.)),
                        ),
                ),
        )
        .child(compact_action(
            "Terminal →",
            Variant::Secondary,
            move |_| ui.open_terminal(id_btn.clone()),
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
    let profile_tag = row.request.as_ref().map(|r| format!("@{}", r.profile.id));
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

    let main = nsh_base::button()
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

    if crate::terminals::model::can_resume(&row, &ui.profiles.get()) {
        let id = row.id.clone();
        actions = actions.child(
            compact_action("Resume Agent", Variant::Quiet, move |_| {
                ui.overview_selection.set(id.clone());
                crate::terminals::navigation::resume(ui, &id)
            })
            .disabled(move || ui.pending.get()),
        );
    }
    if row.reasons.iter().any(|r| r.source != "runtime")
        && crate::terminals::model::binding(&row).is_some()
    {
        let id = row.id.clone();
        actions = actions.child(compact_action("Terminal", Variant::Quiet, move |_| {
            ui.overview_selection.set(id.clone());
            ui.open_terminal(id.clone())
        }));
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
                .tooltip(
                    nsh_base::tooltip()
                        .px_3()
                        .py_2()
                        .rounded(px(RADIUS))
                        .bg(SURFACE)
                        .border_1()
                        .border_color(BORDER)
                        .child(label(hint)),
                )
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
        let latest = items
            .iter()
            .filter(|i| i.card_id == id && i.unread)
            .max_by_key(|i| i.sequence);
        let message = latest
            .map(|i| i.message.lines().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let sequence = latest.map(|i| i.sequence);
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
                .gap_2()
                .py_0p5()
                .min_w(px(0.))
                .border_b_1()
                .border_color(BORDER)
                .child(
                    nsh_base::button()
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
                                                .max_w(px(200.))
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
                .child(compact_action(
                    "Read reply →",
                    Variant::Secondary,
                    move |_| {
                        ui.open_card(id_btn.clone());
                        ui.card_event.set(sequence);
                    },
                )),
        );
    }

    Some(
        section_group("Updates")
            .count(cards.len())
            .tone(Tone::Info)
            .child(rows),
    )
}
