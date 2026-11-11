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
pub fn screen(ui: Ui) -> impl IntoElement {
    let focus_restored = stored(false);
    view()
        .flex_1()
        .min_h(px(0.))
        .p_4()
        .gap_4()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_3()
                .child(heading("Overview"))
                .child(view().flex_1())
                .child(muted(move || {
                    let s = ui.snapshot.get();
                    format!(
                        "Execution slots {} / {} · Queued {}",
                        canban_scheduler::active_count(&s),
                        ui.max_agents.get(),
                        s.requests.iter().filter(|r| r.state == "queued").count()
                    )
                })),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .content_offset(scroll_position(ui, "overview:list"))
                .on_scroll(move |e: &ScrollEvent| capture_scroll(ui, "overview:list".into(), e))
                .child(dynamic(move || {
                    let s = ui.snapshot.get();
                    let mut rows = overview(&s);
                    // Stable task/profile order rather than continuously changing activity timestamps.
                    rows.sort_by(|a, b| {
                        (&a.project_name, a.card.number, &a.id).cmp(&(
                            &b.project_name,
                            b.card.number,
                            &b.id,
                        ))
                    });
                    let mut content = view().gap_4().min_w(px(0.));
                    let mut updates = unread_updates(ui);
                    for group in ["Needs you", "Running", "Queued", "Idle", "Closed"] {
                        if group == "Running"
                            && let Some(v) = updates.take()
                        {
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
                        let mut section = view().gap_1().min_w(px(0.));
                        let collapsed = group == "Closed" && !ui.overview_closed.get()
                            || group == "Idle" && !ui.overview_idle.get();
                        if matches!(group, "Idle" | "Closed") {
                            section = section.child(
                                action(
                                    format!(
                                        "{} {group} · {count}",
                                        if collapsed { "▸" } else { "▾" }
                                    ),
                                    Variant::Quiet,
                                    move |_| {
                                        if group == "Closed" {
                                            ui.overview_closed.update(|v| *v = !*v);
                                        } else {
                                            ui.overview_idle.update(|v| *v = !*v);
                                        }
                                    },
                                )
                                .self_start(),
                            );
                        } else {
                            section =
                                section.child(label(format!("{group} · {count}")).text_color(
                                    if group == "Needs you" {
                                        ATTENTION
                                    } else {
                                        MUTED
                                    },
                                ));
                        }
                        if !collapsed {
                            for row in members {
                                let selected = ui.overview_selection.get() == row.id;
                                section =
                                    section.child(row_view(ui, row, selected, focus_restored));
                            }
                            for entry in configurations {
                                let id = entry.saved.id.clone();
                                section = section.child(
                                    nsh_base::button()
                                        .w_full()
                                        .items_start()
                                        .p_3()
                                        .gap_1()
                                        .bg(CLEAR)
                                        .hover(|s| s.bg(CARD_HOVER))
                                        .child(label(format!(
                                            "Edit Project Workflows · {}",
                                            entry.project_name
                                        )))
                                        .child(muted(format!(
                                            "Configuration · {} · Terminal →",
                                            entry.display_status()
                                        )))
                                        .on_press(move |_: &()| ui.open_terminal(id.clone())),
                                );
                            }
                        }
                        content = content.child(section);
                    }
                    if rows.is_empty() && s.configuration_sessions.is_empty() {
                        content = content.child(muted(
                            "No Agent activity. Start work from a Card or Project Workflows.",
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
fn row_view(
    ui: Ui,
    row: OverviewRow,
    selected: bool,
    focus_restored: Stored<bool>,
) -> impl IntoElement {
    let reference = element_ref();
    let activate_row = row.clone();
    let text = summary(&row);
    let profile = row
        .request
        .as_ref()
        .map(|r| format!(" · @{}", r.profile.id))
        .unwrap_or_default();
    let metadata = if profile.is_empty() {
        row.project_name.clone()
    } else {
        format!(
            "{} · {}",
            profile.trim_start_matches(" · "),
            row.project_name
        )
    };
    let unread = ui
        .reminder_counts
        .with(|m| m.get(&row.card.id).map(|c| c.unread).unwrap_or(0));
    let hint = format!(
        "#{} {}\n{metadata}\n{text}",
        row.card.number, row.card.title
    );
    let mut main = nsh_base::button()
        .element_ref(&reference)
        .flex_1()
        .min_w(px(0.))
        .items_start()
        .gap_1()
        .p_2()
        .bg(CLEAR)
        .hover(|s| s.bg(CARD_HOVER))
        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
        .accessibility_label(format!(
            "#{} {} · {metadata} · {} · {}",
            row.card.number,
            row.card.title,
            text,
            primary(&row)
        ))
        .child(
            view()
                .flex_row()
                .w_full()
                .items_center()
                .gap_3()
                .child(
                    label(format!(
                        "{} #{} {}",
                        if row.actionable() { "!" } else { "·" },
                        row.card.number,
                        row.card.title
                    ))
                    .flex_1()
                    .min_w(px(0.))
                    .truncate(),
                )
                .child(
                    muted(metadata)
                        .max_w(px(if ui.narrow.get() { 140. } else { 300. }))
                        .min_w(px(0.))
                        .truncate(),
                ),
        )
        .child(
            muted(text)
                .text_color(if row.actionable() { ATTENTION } else { MUTED })
                .truncate()
                .min_w(px(0.)),
        );
    if unread > 0 && row.actionable() {
        main = main.child(muted(format!("{unread} unread")));
    }
    let button_row = row.clone();
    let mut actions = view()
        .items_end()
        .gap_1()
        .flex_shrink_0()
        .child(compact_action(primary(&row), Variant::Quiet, move |_| {
            activate(ui, &button_row)
        }));
    if row.reasons.iter().any(|r| r.source != "runtime")
        && crate::terminals::model::binding(&row).is_some()
    {
        let id = row.id.clone();
        actions = actions.child(compact_action("Terminal →", Variant::Quiet, move |_| {
            ui.overview_selection.set(id.clone());
            ui.open_terminal(id.clone())
        }));
    }
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
    if let Some(since) = row
        .attempt
        .as_ref()
        .and_then(|a| a.lifecycle.waiting.as_ref().map(|w| w.since))
    {
        actions = actions.child(muted(format!(
            "Waiting {}",
            crate::components::activity::age(since).trim_end_matches(" ago")
        )));
    }
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .border_b_1()
        .border_color(BORDER)
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
                .tooltip(nsh_base::tooltip().child(label(hint)))
                .child(main.on_press(move |_: &()| activate(ui, &activate_row))),
        )
        .child(actions)
}
fn unread_updates(ui: Ui) -> Option<View> {
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
        .filter(|c| counts.get(&c.id).is_some_and(|n| n.unread > 0) && !actionable.contains(&c.id))
        .collect();
    if cards.is_empty() {
        return None;
    }
    let mut section = view()
        .gap_1()
        .child(label(format!("Updates · {}", cards.len())).text_color(MUTED));
    for card in cards {
        let id = card.id.clone();
        let latest = items
            .iter()
            .filter(|i| i.card_id == id && i.unread)
            .max_by_key(|i| i.sequence);
        let message = latest.map(|i| i.message.clone()).unwrap_or_default();
        let sequence = latest.map(|i| i.sequence);
        let project = s
            .projects
            .iter()
            .find(|p| p.id == card.project_id)
            .map(|p| p.name.as_str())
            .unwrap_or("");
        section = section.child(
            nsh_base::button()
                .w_full()
                .items_start()
                .p_2()
                .gap_1()
                .bg(CLEAR)
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                .child(
                    label(format!("• #{} {} · {}", card.number, card.title, project))
                        .truncate()
                        .min_w(px(0.)),
                )
                .child(muted(message).truncate().min_w(px(0.)))
                .child(muted(format!(
                    "{} unread · Read reply →",
                    counts[&id].unread
                )))
                .on_press(move |_: &()| {
                    ui.open_card(id.clone());
                    ui.card_event.set(sequence);
                }),
        );
    }
    Some(section)
}
