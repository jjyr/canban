//! Global Activity & Notifications popover (⌘I) + shared Overview rows.
use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_notifications::{OverviewRow, overview};
pub use canban_ui_kit::{Icon, Tone};
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

fn apply_card_read_local(ui: Ui, card_id: &str, sequence: u64) {
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
                id: card_id.to_owned(),
                project_id: pid,
                card_id: card_id.to_owned(),
                sequence,
            });
        }
    });
}

fn apply_all_cards_read_local(ui: Ui, project_scope: Option<&str>) -> Vec<canban_types::CardRead> {
    let mut updated = Vec::new();
    ui.snapshot.update(|s| {
        let cards: Vec<_> = s
            .cards
            .iter()
            .filter(|c| {
                c.removed_at.is_none() && project_scope.is_none_or(|pid| c.project_id == pid)
            })
            .map(|c| (c.id.clone(), c.project_id.clone()))
            .collect();
        for (cid, pid) in cards {
            let max_seq = s
                .card_summaries
                .get(&cid)
                .map(|summary| summary.last_event_seq)
                .unwrap_or(0);
            if max_seq == 0 {
                continue;
            }
            let read = if let Some(existing) = s.card_reads.iter_mut().find(|r| r.card_id == cid) {
                existing.sequence = existing.sequence.max(max_seq);
                existing.clone()
            } else {
                let entry = canban_types::CardRead {
                    id: cid.clone(),
                    project_id: pid,
                    card_id: cid,
                    sequence: max_seq,
                };
                s.card_reads.push(entry.clone());
                entry
            };
            updated.push(read);
        }
    });
    updated
}

pub fn mark_card_read(ui: Ui, card_id: String, sequence: u64) {
    if !crate::backend::is_connected() {
        apply_card_read_local(ui, &card_id, sequence);
        return;
    }
    if ui.is_method_pending("notifications.read") {
        return;
    }
    let prev = ui
        .snapshot
        .with_untracked(|s| s.card_reads.iter().find(|r| r.card_id == card_id).cloned());
    apply_card_read_local(ui, &card_id, sequence);
    let rollback_id = card_id.clone();
    dispatch_complete(
        ui,
        "notifications.read",
        json!({"card_id": card_id, "sequence": sequence}),
        move |res| {
            if res.is_err() {
                ui.snapshot.update(|s| {
                    if let Some(prev_read) = prev {
                        if let Some(cur) =
                            s.card_reads.iter_mut().find(|r| r.card_id == rollback_id)
                        {
                            *cur = prev_read;
                        } else {
                            s.card_reads.push(prev_read);
                        }
                    } else {
                        s.card_reads.retain(|r| r.card_id != rollback_id);
                    }
                });
            }
        },
    );
}

pub fn mark_all_read(ui: Ui, project_scope: Option<String>) {
    if !crate::backend::is_connected() {
        apply_all_cards_read_local(ui, project_scope.as_deref());
        ui.notify("Marked updates read");
        return;
    }
    if ui.is_method_pending("notifications.read_all") {
        return;
    }
    let prev_reads = ui.snapshot.with_untracked(|s| s.card_reads.clone());
    let updated_reads = apply_all_cards_read_local(ui, project_scope.as_deref());
    dispatch_complete(
        ui,
        "notifications.read_all",
        json!({"project_id": project_scope}),
        move |res| match res {
            Ok(_) => {
                let _ = ui
                    .committed_writes
                    .try_with(|w| w.record_card_reads(&updated_reads));
                ui.notify("Marked updates read");
            }
            Err(_) => {
                ui.snapshot.update(|s| s.card_reads = prev_reads);
            }
        },
    );
}

pub fn standalone_terminal_needs_you(
    s: &canban_types::Snapshot,
    e: &crate::terminals::Entry,
) -> bool {
    e.agent.is_none()
        && e.listed()
        && (e.needs_attention()
            || e.herdr_status().is_attention()
            || (!e.ended()
                && s.scheduled_runs
                    .iter()
                    .any(|r| r.id == e.saved.id && r.state == "blocked")))
}

pub fn standalone_terminal_is_running(
    s: &canban_types::Snapshot,
    e: &crate::terminals::Entry,
) -> bool {
    if e.agent.is_some() || !e.listed() || standalone_terminal_needs_you(s, e) {
        return false;
    }
    let is_special_active = s
        .configuration_sessions
        .iter()
        .any(|c| c.id == e.saved.id && !matches!(c.state.as_str(), "finished" | "cancelled"))
        || s.merge_batches
            .iter()
            .any(|b| b.id == e.saved.id && b.active())
        || s.scheduled_runs
            .iter()
            .any(|r| r.id == e.saved.id && r.active());
    e.herdr_status().is_working()
        || (is_special_active
            && !matches!(
                e.herdr_status(),
                canban_types::HerdrStatus::Done
                    | canban_types::HerdrStatus::Idle
                    | canban_types::HerdrStatus::Ended
            ))
}

pub fn running_agents_count_from(
    s: &canban_types::Snapshot,
    rows: &[OverviewRow],
    entries: &[crate::terminals::Entry],
    scope: Option<&str>,
) -> usize {
    let card_running = rows
        .iter()
        .filter(|r| {
            scope.is_none_or(|pid| r.card.project_id == pid)
                && r.group == "Working"
                && !r.actionable()
        })
        .count();
    let standalone_running = entries
        .iter()
        .filter(|e| {
            scope.is_none_or(|pid| e.project_id.as_deref() == Some(pid))
                && standalone_terminal_is_running(s, e)
        })
        .count();
    card_running + standalone_running
}

fn attention_summary(ui: Ui) -> (usize, usize, usize) {
    let all_rows = ui.overview_rows.get();
    let actionable_cards: std::collections::BTreeSet<&str> = all_rows
        .iter()
        .filter(|r| r.actionable())
        .map(|r| r.card.id.as_str())
        .collect();
    let (terminal_alerts, standalone_running) = ui.snapshot.with(|s| {
        ui.terminal_entries.with(|entries| {
            let alerts = entries
                .iter()
                .filter(|e| standalone_terminal_needs_you(s, e))
                .count();
            let running = entries
                .iter()
                .filter(|e| standalone_terminal_is_running(s, e))
                .count();
            (alerts, running)
        })
    });
    let actionable = actionable_cards.len() + terminal_alerts;
    let card_running = all_rows
        .iter()
        .filter(|r| r.group == "Working" && !r.actionable())
        .count();
    let running = card_running + standalone_running;
    let unread_cards = ui.snapshot.with(|s| {
        ui.reminder_counts.with(|counts| {
            s.cards
                .iter()
                .filter(|c| {
                    counts.get(&c.id).is_some_and(|n| n.unread > 0)
                        && !actionable_cards.contains(c.id.as_str())
                })
                .count()
        })
    });
    (actionable, unread_cards, running)
}

pub fn trigger_pill(ui: Ui) -> impl IntoElement {
    let stats = computed(move || attention_summary(ui));
    let is_open = computed(move || ui.utility_popup.get() == "notifications");
    let has_alerts = computed(move || {
        let (actionable, unread, _) = stats.get();
        actionable + unread > 0
    });
    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(
            hint_tooltip(move || {
                let (actionable, unread, running) = stats.get();
                format!(
                    "Activity & Notifications · {}\n{} needs you · {} unread · {} running",
                    crate::commands::shortcuts::NOTIFICATIONS_DISPLAY,
                    actionable,
                    unread,
                    running
                )
            })
            .placement(Placement::Bottom),
        )
        .child(
            canban_ui_kit::press_target()
                .h(px(28.))
                .px_2p5()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(move || {
                    if is_open.get() {
                        ACCENT
                    } else if stats.get().0 > 0 {
                        ATTENTION
                    } else {
                        BORDER
                    }
                })
                .bg(move || {
                    if is_open.get() {
                        SELECTED
                    } else if stats.get().0 > 0 {
                        ATTENTION_SURFACE
                    } else {
                        INSET
                    }
                })
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                .flex_row()
                .items_center()
                .gap_1p5()
                .accessibility_label(move || {
                    let (actionable, unread, running) = stats.get();
                    Some(
                        format!(
                            "Activity & Notifications, {} alerts, {} running",
                            actionable + unread,
                            running
                        )
                        .into(),
                    )
                })
                .on_press(move |_: &()| toggle(ui))
                .child(
                    icon_tinted(Icon::Overview, move || {
                        let (actionable, unread, _) = stats.get();
                        if actionable > 0 {
                            ATTENTION
                        } else if unread > 0 || is_open.get() {
                            ACCENT
                        } else {
                            MUTED
                        }
                    })
                    .w(px(13.5))
                    .h(px(13.5)),
                )
                .child(when(
                    move || has_alerts.get(),
                    move || {
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .child(
                                label(move || {
                                    let (actionable, unread, _) = stats.get();
                                    (actionable + unread).to_string()
                                })
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || {
                                    if stats.get().0 > 0 { ATTENTION } else { TEXT }
                                }),
                            )
                            .child(muted("·").text_size(px(11.)))
                    },
                ))
                .child(
                    view()
                        .w(px(6.))
                        .h(px(6.))
                        .rounded_full()
                        .bg(move || if stats.get().2 > 0 { SUCCESS } else { MUTED }),
                )
                .child(
                    muted(move || stats.get().2.to_string())
                        .text_size(px(11.5))
                        .font_family(canban_ui_kit::tokens::MONO_FONT),
                ),
        )
}

pub fn popover(ui: Ui) -> impl IntoElement {
    let project_scope = signal(String::new());
    let valid_scope = computed(move || {
        let scope = project_scope.get();
        if scope.is_empty() {
            return None;
        }
        let known = canban_types::is_global_project_id(&scope)
            || ui
                .snapshot
                .with(|s| s.projects.iter().any(|p| p.id == scope));
        known.then_some(scope)
    });
    let running_count = computed(move || {
        let scope = valid_scope.get();
        ui.snapshot.with(|s| {
            ui.overview_rows.with(|all_rows| {
                ui.terminal_entries.with(|entries| {
                    running_agents_count_from(s, all_rows, entries, scope.as_deref())
                })
            })
        })
    });
    let has_running = computed(move || running_count.get() > 0);
    let queued_count = computed(move || {
        let scope = valid_scope.get();
        ui.snapshot.with(|s| {
            s.terminals
                .iter()
                .filter(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        scope.as_deref().is_none_or(|pid| a.project_id == pid)
                            && (a.state == "queued" || a.pending.is_some())
                    })
                })
                .count()
        })
    });
    let has_queued = computed(move || queued_count.get() > 0);
    let has_unread = computed(move || {
        let scope = valid_scope.get();
        ui.snapshot.with(|s| {
            ui.reminder_counts.with(|counts| {
                s.cards.iter().any(|c| {
                    scope.as_deref().is_none_or(|pid| c.project_id == pid)
                        && counts.get(&c.id).is_some_and(|n| n.unread > 0)
                })
            })
        })
    });
    let filter_chips = computed(move || -> Option<Vec<(String, String)>> {
        ui.snapshot.with(|s| {
            ui.reminder_counts.with(|counts| {
                ui.overview_rows.with(|all_rows| {
                    ui.terminal_entries.with(|entries| {
                        let has_global = entries.iter().any(|e| {
                            e.listed()
                                && e.project_id
                                    .as_deref()
                                    .is_some_and(canban_types::is_global_project_id)
                        }) || s.scheduled_runs.iter().any(|r| {
                            canban_types::is_global_project_id(&r.project_id)
                                && (r.active() || r.state == "blocked")
                        });
                        if s.projects.len() <= 1 && !has_global {
                            return None;
                        }
                        let mut chips = vec![(String::new(), "All".to_owned())];
                        if has_global {
                            let global_alerts = entries
                                .iter()
                                .filter(|e| {
                                    e.project_id
                                        .as_deref()
                                        .is_some_and(canban_types::is_global_project_id)
                                        && standalone_terminal_needs_you(s, e)
                                })
                                .count();
                            let glabel = if global_alerts > 0 {
                                format!("Global · {global_alerts}")
                            } else {
                                "Global".into()
                            };
                            chips.push((canban_types::GLOBAL_PROJECT_ID.into(), glabel));
                        }
                        for p in &s.projects {
                            let pid = p.id.clone();
                            let actionable_cards: std::collections::BTreeSet<_> = all_rows
                                .iter()
                                .filter(|r| r.card.project_id == pid && r.actionable())
                                .map(|r| r.card.id.clone())
                                .collect();
                            let proj_alerts = all_rows
                                .iter()
                                .filter(|r| r.card.project_id == pid && r.actionable())
                                .count()
                                + entries
                                    .iter()
                                    .filter(|e| {
                                        e.project_id.as_deref() == Some(pid.as_str())
                                            && standalone_terminal_needs_you(s, e)
                                    })
                                    .count()
                                + s.cards
                                    .iter()
                                    .filter(|c| {
                                        c.project_id == pid
                                            && counts.get(&c.id).is_some_and(|n| n.unread > 0)
                                            && !actionable_cards.contains(&c.id)
                                    })
                                    .count();
                            let hid = s.project_host(&pid);
                            let base_name = if hid != "local" && s.has_remote_hosts() {
                                format!("{} ({})", p.name, s.host_display_name(hid))
                            } else {
                                p.name.clone()
                            };
                            let seg_label = if proj_alerts > 0 {
                                format!("{base_name} · {proj_alerts}")
                            } else {
                                base_name
                            };
                            chips.push((pid, seg_label));
                        }
                        Some(chips)
                    })
                })
            })
        })
    });
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
                        .w(px(520.))
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
                                .child(
                                    view()
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
                                                .child(when(
                                                    move || has_running.get(),
                                                    move || {
                                                        status(
                                                            move || {
                                                                format!(
                                                                    "{} running",
                                                                    running_count.get()
                                                                )
                                                            },
                                                            Tone::Info,
                                                        )
                                                    },
                                                ))
                                                .child(when(
                                                    move || !has_running.get(),
                                                    move || status("0 running", Tone::Neutral),
                                                ))
                                                .child(when(
                                                    move || has_queued.get(),
                                                    move || {
                                                        badge(move || {
                                                            format!("{} queued", queued_count.get())
                                                        })
                                                    },
                                                )),
                                        )
                                        .child(
                                            view()
                                                .flex_row()
                                                .items_center()
                                                .gap_1()
                                                .child(when(
                                                    move || has_unread.get(),
                                                    move || {
                                                        compact_action(
                                                            "✓ Read all",
                                                            Variant::Quiet,
                                                            move |_| {
                                                                mark_all_read(
                                                                    ui,
                                                                    valid_scope.get_untracked(),
                                                                )
                                                            },
                                                        )
                                                    },
                                                ))
                                                .child(chrome_button(
                                                    Icon::Close,
                                                    "Close Activity & Notifications",
                                                    move |_| close(ui),
                                                )),
                                        ),
                                )
                                .child(when(
                                    move || filter_chips.with(Option::is_some),
                                    move || {
                                        dynamic(move || {
                                            let Some(chips) = filter_chips.get() else {
                                                return view().into_element();
                                            };
                                            let mut filter_seg = segmented();
                                            for (scope_id, seg_label) in chips {
                                                let id_sel = scope_id.clone();
                                                let id_press = scope_id;
                                                filter_seg = filter_seg.child(compact_segment(
                                                    seg_label,
                                                    None,
                                                    move || {
                                                        valid_scope.with(|v| {
                                                            if id_sel.is_empty() {
                                                                v.is_none()
                                                            } else {
                                                                v.as_deref()
                                                                    == Some(id_sel.as_str())
                                                            }
                                                        })
                                                    },
                                                    move |_| project_scope.set(id_press.clone()),
                                                ));
                                            }
                                            scroll()
                                                .horizontal()
                                                .scrollbar(false)
                                                .child(filter_seg)
                                                .into_element()
                                        })
                                    },
                                )),
                        )
                        .child(scroll().max_h(px(560.)).p_3p5().child(activity_panel(
                            ui,
                            project_scope,
                            stored(true),
                            true,
                        ))),
                ),
        )
}

fn destination(ui: Ui, row: &OverviewRow, review: bool) {
    close(ui);
    ui.open_card(row.card.id.clone());
    if let Some(id) = row.outcome_terminal_id() {
        ui.request.set(id.into());
    }
    if review {
        ui.page.set("review".into());
    }
}

pub fn show_card_attention(ui: Ui, card: &str) {
    let snapshot = ui.snapshot.get_untracked();
    let rows: Vec<_> = overview(&snapshot)
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
            let (_, _, headline, detail) = classify_reason(&snapshot, &row, reason);
            let menu_label = match detail {
                Some(d) => format!("{headline} · {d}"),
                None => headline.into(),
            };
            items.push(MenuItem::new(menu_label).on_select(move || {
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

fn runtime_blocked_with_terminal(row: &OverviewRow) -> bool {
    row.reasons.iter().any(|r| r.source == "runtime")
        && row.agent().map(|a| a.runtime_status()) == Some("blocked")
        && crate::terminals::model::binding(row).is_some()
}

fn activate(ui: Ui, row: &OverviewRow) {
    ui.overview_selection.set(row.id.clone());
    close(ui);
    if runtime_blocked_with_terminal(row) {
        ui.open_card_with_terminal(row.card.id.clone(), Some(row.id.clone()));
    } else if row.reasons.iter().any(|r| r.source == "review") {
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
    if runtime_blocked_with_terminal(row) {
        "Card & Terminal →"
    } else if row.reasons.iter().any(|r| r.source == "review") {
        "Review →"
    } else if row.reasons.iter().any(|r| r.source != "runtime") {
        "Card →"
    } else if crate::terminals::model::binding(row).is_some() {
        "Card & Terminal →"
    } else if crate::terminals::model::closed(row) {
        "History →"
    } else {
        "Card →"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowEvent {
    pub icon: Icon,
    pub tone: Tone,
    pub live: bool,
    pub headline: String,
    pub detail: Option<String>,
}

fn single_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_machine_prefix(s: &str) -> String {
    let mut text = single_line(s);
    for prefix in [
        "completed: ",
        "blocked: ",
        "failed: ",
        "backlog: ",
        "Returned to Backlog: ",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim().to_owned();
            break;
        }
    }
    if matches!(
        text.as_str(),
        "completed" | "blocked" | "failed" | "backlog" | "Returned to Backlog"
    ) {
        return String::new();
    }
    text
}

fn clean_wait_detail(title: &str, message: &str) -> Option<String> {
    let t = single_line(title);
    let m = single_line(message);
    let generic_title = t.is_empty()
        || t.eq_ignore_ascii_case("agent needs input")
        || t.eq_ignore_ascii_case("permission required")
        || t.eq_ignore_ascii_case("input required")
        || t.eq_ignore_ascii_case("sign-in required")
        || t.eq_ignore_ascii_case("answer required");
    let combined = match (generic_title, m.is_empty()) {
        (true, true) => String::new(),
        (true, false) => m,
        (false, true) => t,
        (false, false) => {
            if m.to_lowercase().contains(&t.to_lowercase()) {
                m
            } else if t.to_lowercase().contains(&m.to_lowercase()) {
                t
            } else {
                format!("{t} · {m}")
            }
        }
    };
    (!combined.is_empty()).then_some(combined)
}

fn classify_runtime_wait(
    w: &canban_types::RuntimeWait,
) -> (Icon, Tone, &'static str, Option<String>) {
    let detail = clean_wait_detail(&w.title, &w.message);
    match w.kind.as_str() {
        "permission" | "approval" | "command" | "tool" | "bash" | "file" => (
            Icon::ShieldAlert,
            Tone::Attention,
            "Permission required",
            detail,
        ),
        "auth" | "login" | "signin" | "sign_in" => (
            Icon::CircleHelp,
            Tone::Attention,
            "Sign-in required",
            detail,
        ),
        "question" | "prompt" | "answer" => {
            (Icon::CircleHelp, Tone::Attention, "Answer needed", detail)
        }
        _ => (Icon::CircleHelp, Tone::Attention, "Input needed", detail),
    }
}

fn is_merge_gate_reason(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("merge")
        || lower.contains("conflict")
        || lower.contains("upstream")
        || lower.contains("fast-forward")
        || lower.contains("diverged")
}

fn classify_reason(
    snapshot: &canban_types::Snapshot,
    row: &OverviewRow,
    reason: &canban_notifications::OverviewReason,
) -> (Icon, Tone, &'static str, Option<String>) {
    classify_reason_at(snapshot, row, reason, canban_types::now())
}

fn classify_reason_at(
    snapshot: &canban_types::Snapshot,
    row: &OverviewRow,
    reason: &canban_notifications::OverviewReason,
    now_ms: u64,
) -> (Icon, Tone, &'static str, Option<String>) {
    match reason.source.as_str() {
        "runtime" => {
            let runtime = row.agent().map(|a| a.runtime_status_at(now_ms));
            if runtime == Some("blocked") {
                if let Some(w) = row.agent().and_then(|a| a.lifecycle.waiting.as_ref()) {
                    classify_runtime_wait(w)
                } else {
                    (
                        Icon::CircleHelp,
                        Tone::Attention,
                        "Input needed",
                        Some("Waiting in terminal".into()),
                    )
                }
            } else if runtime == Some("error") {
                (
                    Icon::TriangleAlert,
                    Tone::Danger,
                    "Terminal error",
                    Some("Inspect live terminal".into()),
                )
            } else if row.agent().is_some_and(|a| a.trigger.is_automatic()) {
                (
                    Icon::TriangleAlert,
                    Tone::Danger,
                    "Stopped before reporting",
                    Some("Inspect terminal".into()),
                )
            } else {
                (
                    Icon::CircleHelp,
                    Tone::Attention,
                    "Waiting for your input",
                    Some("Agent finished its turn".into()),
                )
            }
        }
        "review" => {
            let outcome_id = row.outcome_terminal_id();
            let report_detail = outcome_id
                .and_then(|tid| {
                    snapshot
                        .reports
                        .iter()
                        .rev()
                        .find(|r| {
                            r.terminal_id.as_deref() == Some(tid) && !r.message.trim().is_empty()
                        })
                        .map(|r| strip_machine_prefix(&r.message))
                })
                .or_else(|| {
                    snapshot
                        .reports
                        .iter()
                        .rev()
                        .find(|r| r.card_id == row.card.id && !r.message.trim().is_empty())
                        .map(|r| strip_machine_prefix(&r.message))
                })
                .filter(|s| !s.is_empty());
            let gate_text = strip_machine_prefix(&reason.message);
            let generic_gate = gate_text.is_empty()
                || gate_text.eq_ignore_ascii_case("agent report ready; human review required")
                || gate_text.eq_ignore_ascii_case("human review required")
                || gate_text.eq_ignore_ascii_case("review changes")
                || gate_text.eq_ignore_ascii_case("ready for review")
                || gate_text.eq_ignore_ascii_case("review required");
            if row.card.hold_auto_merge && !row.card.resolved() || is_merge_gate_reason(&gate_text)
            {
                let headline = if gate_text.to_lowercase().contains("conflict") {
                    "Merge conflict"
                } else {
                    "Merge paused"
                };
                let detail = (!generic_gate).then_some(gate_text).or(report_detail);
                (Icon::GitMerge, Tone::Attention, headline, detail)
            } else {
                let detail = report_detail.or_else(|| (!generic_gate).then_some(gate_text));
                (Icon::Eye, Tone::Attention, "Ready for review", detail)
            }
        }
        "task" => {
            let msg = strip_machine_prefix(&reason.message);
            let outcome_state = row
                .outcome_terminal_id()
                .and_then(|tid| snapshot.terminals.iter().find(|t| t.id == tid))
                .and_then(|t| t.agent.as_ref())
                .map(|a| a.state.as_str());
            if is_merge_gate_reason(&msg) || (row.card.hold_auto_merge && !row.card.resolved()) {
                let is_fail = msg.to_lowercase().contains("fail");
                let headline = if msg.to_lowercase().contains("conflict") {
                    "Merge conflict"
                } else if is_fail {
                    "Merge failed"
                } else {
                    "Merge paused"
                };
                let tone = if is_fail {
                    Tone::Danger
                } else {
                    Tone::Attention
                };
                (
                    Icon::GitMerge,
                    tone,
                    headline,
                    (!msg.is_empty()).then_some(msg),
                )
            } else if msg.starts_with("CI failed") || msg.starts_with("GitHub checks failed") {
                (
                    Icon::TriangleAlert,
                    Tone::Danger,
                    "CI failed",
                    (!msg.is_empty()).then_some(msg),
                )
            } else if msg.starts_with("Changes requested on PR") {
                (
                    Icon::Edit,
                    Tone::Danger,
                    "Changes requested",
                    (!msg.is_empty()).then_some(msg),
                )
            } else if msg.starts_with("Delivery PR was closed")
                || msg.starts_with("PR #") && msg.contains("closed")
            {
                (
                    Icon::Close,
                    Tone::Danger,
                    "PR closed",
                    (!msg.is_empty()).then_some(msg),
                )
            } else if matches!(outcome_state, Some("failed" | "needs_report"))
                || msg.to_lowercase().contains("exited without")
                || msg.to_lowercase().contains("unavailable")
                || msg.to_lowercase().contains("failed")
            {
                (
                    Icon::TriangleAlert,
                    Tone::Danger,
                    "Agent failed",
                    (!msg.is_empty()).then_some(msg),
                )
            } else {
                (
                    Icon::CirclePause,
                    Tone::Attention,
                    "Needs decision",
                    (!msg.is_empty()).then_some(msg),
                )
            }
        }
        "recovery" => {
            let msg = single_line(&reason.message);
            if let Some(a) = row.agent()
                && a.connection_state != "connected"
            {
                let detail = a
                    .error
                    .as_deref()
                    .map(single_line)
                    .filter(|s| !s.is_empty())
                    .or_else(|| Some(format!("Connection {}", a.connection_state)));
                (
                    Icon::TriangleAlert,
                    Tone::Danger,
                    "Agent disconnected",
                    detail,
                )
            } else if msg.to_lowercase().contains("exited") {
                (
                    Icon::TriangleAlert,
                    Tone::Danger,
                    "Agent exited early",
                    Some(msg),
                )
            } else {
                (
                    Icon::TriangleAlert,
                    Tone::Danger,
                    "Recovery needed",
                    (!msg.is_empty()).then_some(msg),
                )
            }
        }
        "question" => {
            let msg = single_line(&reason.message);
            let detail = (!msg.is_empty()
                && !msg.eq_ignore_ascii_case("question delivery uncertain"))
            .then_some(msg);
            (
                Icon::TriangleAlert,
                Tone::Danger,
                "Reply undelivered",
                detail,
            )
        }
        _ => {
            let msg = strip_machine_prefix(&reason.message);
            (
                Icon::Info,
                Tone::Attention,
                "Needs attention",
                (!msg.is_empty()).then_some(msg),
            )
        }
    }
}

fn reason_priority_at(source: &str, row: &OverviewRow, now_ms: u64) -> u8 {
    match source {
        "runtime" => {
            if row.agent().map(|a| a.runtime_status_at(now_ms)) == Some("blocked") {
                0
            } else {
                1
            }
        }
        "recovery" => 2,
        "question" => 3,
        "task" => 4,
        "review" => 5,
        _ => 6,
    }
}

pub fn describe_row(snapshot: &canban_types::Snapshot, row: &OverviewRow) -> RowEvent {
    describe_row_at(snapshot, row, canban_types::now())
}

/// Deterministic clock-injected variant of [`describe_row`].
pub fn describe_row_at(
    snapshot: &canban_types::Snapshot,
    row: &OverviewRow,
    now_ms: u64,
) -> RowEvent {
    if !row.reasons.is_empty() {
        let mut ordered: Vec<_> = row.reasons.iter().collect();
        ordered.sort_by_key(|r| reason_priority_at(&r.source, row, now_ms));
        let (icon, tone, headline, mut detail) =
            classify_reason_at(snapshot, row, ordered[0], now_ms);
        if ordered.len() > 1 {
            let extras: Vec<_> = ordered[1..]
                .iter()
                .map(|r| {
                    let (_, _, h, d) = classify_reason_at(snapshot, row, r, now_ms);
                    match d {
                        Some(d) => format!("{h}: {d}"),
                        None => h.to_owned(),
                    }
                })
                .collect();
            let extra_str = extras.join(" · ");
            detail = Some(match detail {
                Some(d) => format!("{d} · {extra_str}"),
                None => extra_str,
            });
        }
        return RowEvent {
            icon,
            tone,
            live: false,
            headline: headline.into(),
            detail,
        };
    }

    let profile_icon = row
        .agent()
        .map(|a| crate::profiles::provider(&a.profile).0)
        .unwrap_or(Icon::Agent);
    let stage_detail = row
        .agent()
        .filter(|a| !a.stage.is_empty())
        .map(|a| format!("Stage: {}", a.stage));

    if let Some(a) = row.agent() {
        if a.finished() {
            let failed = matches!(a.state.as_str(), "failed" | "needs_report");
            return RowEvent {
                icon: if failed {
                    Icon::TriangleAlert
                } else {
                    Icon::History
                },
                tone: if failed { Tone::Danger } else { Tone::Neutral },
                live: false,
                headline: if failed {
                    "Exited with error".into()
                } else {
                    "Session ended".into()
                },
                detail: a
                    .error
                    .as_deref()
                    .map(single_line)
                    .filter(|s| !s.is_empty())
                    .or_else(|| Some("Saved history available".into())),
            };
        }
        if a.connection_state != "connected" {
            return RowEvent {
                icon: Icon::TriangleAlert,
                tone: Tone::Danger,
                live: false,
                headline: "Disconnected".into(),
                detail: Some("Original terminal retained".into()),
            };
        }
        if row.group == "Working" {
            let live_msg = a
                .observed
                .as_ref()
                .and_then(|v| {
                    v["status"]["msg"]
                        .as_str()
                        .filter(|s| !s.trim().is_empty())
                        .or_else(|| {
                            v["status"]["title"]
                                .as_str()
                                .filter(|s| !s.trim().is_empty())
                        })
                })
                .map(single_line);
            return RowEvent {
                icon: profile_icon,
                tone: Tone::Info,
                live: true,
                headline: "Working".into(),
                detail: live_msg.or(stage_detail),
            };
        }
    }

    match row.group.as_str() {
        "Working" => RowEvent {
            icon: profile_icon,
            tone: Tone::Info,
            live: true,
            headline: "Starting".into(),
            detail: stage_detail,
        },
        "Queued" => RowEvent {
            icon: Icon::History,
            tone: Tone::Neutral,
            live: false,
            headline: "Queued".into(),
            detail: Some("Waiting for open slot".into()),
        },
        "Idle" => RowEvent {
            icon: profile_icon,
            tone: Tone::Neutral,
            live: false,
            headline: "Idle".into(),
            detail: Some("Ready for follow-up".into()),
        },
        _ => RowEvent {
            icon: Icon::History,
            tone: Tone::Neutral,
            live: false,
            headline: "Closed".into(),
            detail: Some("Saved conversation".into()),
        },
    }
}

pub fn describe_unread_update(latest: Option<&canban_notifications::Reminder>) -> RowEvent {
    let raw_msg = latest.map(|i| i.message.as_str()).unwrap_or("");
    let matched_event = latest.and_then(|rem| rem.event.as_ref());
    let agent_tag = matched_event
        .and_then(|e| e.actor.strip_prefix("agent:"))
        .map(|a| format!("@{a}"));

    if let Some(ev) = matched_event {
        if ev.kind == "github.pr.ci_succeeded" {
            let number = ev.pr_number.unwrap_or(0);
            let headline = if number > 0 {
                format!("CI passed · PR #{number}")
            } else {
                "CI passed".into()
            };
            let detail = ev
                .summary
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    let s = strip_machine_prefix(raw_msg);
                    (!s.is_empty()).then_some(s)
                });
            return RowEvent {
                icon: Icon::Check,
                tone: Tone::Success,
                live: false,
                headline,
                detail,
            };
        }
        if ev.kind == "github.pr.ci_failed" {
            let number = ev.pr_number.unwrap_or(0);
            let headline = if number > 0 {
                format!("CI failed · PR #{number}")
            } else {
                "CI failed".into()
            };
            let detail = ev
                .summary
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    let s = strip_machine_prefix(raw_msg);
                    (!s.is_empty()).then_some(s)
                });
            return RowEvent {
                icon: Icon::TriangleAlert,
                tone: Tone::Danger,
                live: false,
                headline,
                detail,
            };
        }
        if ev.kind == "github.pr.updated" {
            let number = ev.pr_number.unwrap_or(0);
            let headline = if number > 0 {
                format!("PR #{number} updated")
            } else {
                "PR updated".into()
            };
            let detail = ev
                .summary
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    let s = strip_machine_prefix(raw_msg);
                    (!s.is_empty()).then_some(s)
                });
            return RowEvent {
                icon: Icon::Branch,
                tone: Tone::Info,
                live: false,
                headline,
                detail,
            };
        }
        if ev.kind == "github.pr.commented" {
            let author = ev
                .actor
                .strip_prefix("github:")
                .map(|g| format!("@{g}"))
                .unwrap_or_else(|| ev.actor.clone());
            let number = ev.pr_number.unwrap_or(0);
            let headline = if number > 0 {
                format!("{author} commented · PR #{number}")
            } else {
                format!("{author} commented on PR")
            };
            let detail = ev
                .summary
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    let s = strip_machine_prefix(raw_msg);
                    (!s.is_empty()).then_some(s)
                });
            return RowEvent {
                icon: Icon::GitHub,
                tone: Tone::Info,
                live: false,
                headline,
                detail,
            };
        }
        if ev.kind == "github.pr.reviewed" {
            let author = ev
                .actor
                .strip_prefix("github:")
                .map(|g| format!("@{g}"))
                .unwrap_or_else(|| ev.actor.clone());
            let number = ev.pr_number.unwrap_or(0);
            let state = ev.state.as_deref().unwrap_or("");
            let (icon, tone, verb) = match state {
                "APPROVED" => (Icon::Check, Tone::Success, "approved"),
                "CHANGES_REQUESTED" => (Icon::Edit, Tone::Danger, "requested changes on"),
                "DISMISSED" => (Icon::Info, Tone::Info, "dismissed review on"),
                _ => (Icon::GitHub, Tone::Info, "reviewed"),
            };
            let headline = if number > 0 {
                format!("{author} {verb} PR #{number}")
            } else {
                format!("{author} {verb} PR")
            };
            let detail = ev
                .summary
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    let s = strip_machine_prefix(raw_msg);
                    (!s.is_empty()).then_some(s)
                });
            return RowEvent {
                icon,
                tone,
                live: false,
                headline,
                detail,
            };
        }
        if ev.kind == "github.pr.state_changed" {
            let number = ev.pr_number.unwrap_or(0);
            let to = ev.state.as_deref().unwrap_or("updated");
            let state_label = match to {
                "ready_for_review" => "ready for review",
                _ => to,
            };
            let tone = match to {
                "closed" => Tone::Danger,
                "ready_for_review" | "ready" | "reopened" => Tone::Success,
                _ => Tone::Info,
            };
            let icon = match to {
                "closed" => Icon::Close,
                "ready_for_review" | "ready" | "reopened" => Icon::Check,
                _ => Icon::GitHub,
            };
            let headline = if number > 0 {
                format!("PR #{number} {state_label}")
            } else {
                format!("PR {state_label}")
            };
            let s = strip_machine_prefix(raw_msg);
            return RowEvent {
                icon,
                tone,
                live: false,
                headline,
                detail: (!s.is_empty()).then_some(s),
            };
        }
        if ev.kind == "comment.replied" {
            let headline = match &agent_tag {
                Some(tag) => format!("{tag} replied"),
                None => "Agent replied".into(),
            };
            let detail = strip_machine_prefix(raw_msg);
            return RowEvent {
                icon: Icon::MessageSquare,
                tone: Tone::Info,
                live: false,
                headline,
                detail: (!detail.is_empty()).then_some(detail),
            };
        }
        if ev.kind == "execution.reported" {
            let state = ev.state.as_deref().unwrap_or("completed");
            let detail = strip_machine_prefix(raw_msg);
            let detail_opt = (!detail.is_empty()).then_some(detail);
            return match state {
                "backlog" => RowEvent {
                    icon: Icon::Undo,
                    tone: Tone::Neutral,
                    live: false,
                    headline: "Returned to Backlog".into(),
                    detail: detail_opt,
                },
                "blocked" => RowEvent {
                    icon: Icon::CirclePause,
                    tone: Tone::Attention,
                    live: false,
                    headline: "Agent blocked".into(),
                    detail: detail_opt,
                },
                "failed" => RowEvent {
                    icon: Icon::TriangleAlert,
                    tone: Tone::Danger,
                    live: false,
                    headline: "Agent failed".into(),
                    detail: detail_opt,
                },
                _ => RowEvent {
                    icon: Icon::Check,
                    tone: Tone::Success,
                    live: false,
                    headline: match &agent_tag {
                        Some(tag) => format!("Stage completed · {tag}"),
                        None => "Stage completed".into(),
                    },
                    detail: detail_opt,
                },
            };
        }
    }

    let detail = strip_machine_prefix(raw_msg);
    RowEvent {
        icon: Icon::MessageSquare,
        tone: Tone::Info,
        live: false,
        headline: "New update".into(),
        detail: (!detail.is_empty()).then_some(detail),
    }
}

fn event_headline_row(
    headline: String,
    tone: Tone,
    detail: Option<String>,
    trailing_pill: Option<AnyElement>,
) -> View {
    let headline_color = match tone {
        Tone::Neutral => TEXT,
        other => tone_colors(other).1,
    };
    let mut line = view()
        .flex_row()
        .w_full()
        .items_center()
        .gap_1p5()
        .min_w(px(0.))
        .overflow_hidden()
        .child(
            label(headline)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(headline_color)
                .flex_shrink_0(),
        );
    if let Some(detail) = detail.filter(|d| !d.is_empty()) {
        line = line
            .child(muted("·").flex_shrink_0())
            .child(label(detail).flex_1().min_w(px(0.)).truncate());
    } else {
        line = line.child(view().flex_1().min_w(px(0.)));
    }
    if let Some(pill) = trailing_pill {
        line = line.child(view().flex_shrink_0().child(pill));
    }
    line
}

fn card_context_row(
    card_number: u64,
    card_title: String,
    project_name: Option<String>,
    profile_tag: Option<String>,
    trailing_pills: Vec<AnyElement>,
) -> View {
    let mut parts = vec![format!("#{card_number} {card_title}")];
    if let Some(proj) = project_name.filter(|p| !p.is_empty()) {
        parts.push(proj);
    }
    if let Some(profile) = profile_tag.filter(|p| !p.is_empty()) {
        parts.push(profile);
    }
    let mut line = view()
        .flex_row()
        .w_full()
        .items_center()
        .gap_1p5()
        .min_w(px(0.))
        .overflow_hidden()
        .child(muted(parts.join(" · ")).flex_1().min_w(px(0.)).truncate());
    for pill in trailing_pills {
        line = line.child(view().flex_shrink_0().child(pill));
    }
    line
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let focus_restored = stored(false);
    let project_scope = signal(String::new());
    let valid_scope = computed(move || {
        let scope = project_scope.get();
        if scope.is_empty() {
            return None;
        }
        let known = canban_types::is_global_project_id(&scope)
            || ui.index.with(|idx| idx.projects.contains_key(&scope));
        known.then_some(scope)
    });
    let header_counts = computed(move || {
        let scope = valid_scope.get();
        let scope_opt = scope.as_deref();
        ui.snapshot.with(|s| {
            ui.overview_rows.with(|all_rows| {
                ui.terminal_entries.with(|entries| {
                    let actionable_count = all_rows
                        .iter()
                        .filter(|r| {
                            scope_opt.is_none_or(|pid| r.card.project_id == pid) && r.actionable()
                        })
                        .count()
                        + entries
                            .iter()
                            .filter(|e| {
                                scope_opt.is_none_or(|pid| e.project_id.as_deref() == Some(pid))
                                    && standalone_terminal_needs_you(s, e)
                            })
                            .count();
                    let running_count = running_agents_count_from(s, all_rows, entries, scope_opt);
                    let queued_count = s
                        .terminals
                        .iter()
                        .filter(|t| {
                            t.agent.as_ref().is_some_and(|a| {
                                scope_opt.is_none_or(|pid| a.project_id == pid)
                                    && (a.state == "queued" || a.pending.is_some())
                            })
                        })
                        .count();
                    (actionable_count, running_count, queued_count)
                })
            })
        })
    });
    let header_chips = computed(move || -> Option<Vec<(String, String)>> {
        ui.snapshot.with(|s| {
            ui.terminal_entries.with(|entries| {
                let has_global = entries.iter().any(|e| {
                    e.listed()
                        && e.project_id
                            .as_deref()
                            .is_some_and(canban_types::is_global_project_id)
                }) || s.scheduled_runs.iter().any(|r| {
                    canban_types::is_global_project_id(&r.project_id)
                        && (r.active() || r.state == "blocked")
                });
                if s.projects.len() <= 1 && !has_global {
                    return None;
                }
                let mut chips = vec![(String::new(), "All".to_owned())];
                if has_global {
                    chips.push((canban_types::GLOBAL_PROJECT_ID.into(), "Global".into()));
                }
                for p in &s.projects {
                    chips.push((p.id.clone(), s.project_display_name(&p.id)));
                }
                Some(chips)
            })
        })
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(
            view()
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
                        .child(dynamic(move || {
                            let (actionable_count, running_count, queued_count) =
                                header_counts.get();
                            let mut pills = view().flex_row().flex_wrap().items_center().gap_2();
                            if actionable_count > 0 {
                                pills = pills.child(status(
                                    format!("{actionable_count} needs you"),
                                    Tone::Attention,
                                ));
                            }
                            pills = pills.child(status(
                                format!("{running_count} running"),
                                if running_count > 0 {
                                    Tone::Info
                                } else {
                                    Tone::Neutral
                                },
                            ));
                            if queued_count > 0 {
                                pills = pills.child(badge(format!("Queued {queued_count}")));
                            }
                            pills.into_element()
                        })),
                )
                .child(when(
                    move || header_chips.with(Option::is_some),
                    move || {
                        dynamic(move || {
                            let Some(chips) = header_chips.get() else {
                                return view().into_element();
                            };
                            let mut filter_seg = segmented();
                            for (scope_id, seg_label) in chips {
                                let id_sel = scope_id.clone();
                                let id_press = scope_id;
                                filter_seg = filter_seg.child(compact_segment(
                                    seg_label,
                                    None,
                                    move || {
                                        valid_scope.with(|v| {
                                            if id_sel.is_empty() {
                                                v.is_none()
                                            } else {
                                                v.as_deref() == Some(id_sel.as_str())
                                            }
                                        })
                                    },
                                    move |_| project_scope.set(id_press.clone()),
                                ));
                            }
                            filter_seg.into_element()
                        })
                    },
                )),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .content_offset(scroll_position(ui, "overview:list"))
                .on_scroll(move |e: &ScrollEvent| capture_scroll(ui, "overview:list".into(), e))
                .child(activity_panel(ui, project_scope, focus_restored, false)),
        )
}

#[derive(Clone, PartialEq)]
struct ActivityBodyModel {
    sections: Vec<ActivitySection>,
}

#[derive(Clone, PartialEq)]
enum ActivitySection {
    Updates(Vec<PreparedUnreadCard>),
    Group(ActivityGroupModel),
}

#[derive(Clone, PartialEq)]
struct ActivityGroupModel {
    group: &'static str,
    members: Vec<PreparedOverviewRow>,
    configurations: Vec<PreparedStandaloneRow>,
}

#[derive(Clone, PartialEq)]
struct PreparedOverviewRow {
    row: OverviewRow,
    event: RowEvent,
    profile_tag: Option<String>,
    remote_host: Option<String>,
    waiting_since: Option<u64>,
    unread: usize,
    primary_label: &'static str,
    runtime_blocked_with_terminal: bool,
    has_non_runtime_reason: bool,
    has_binding: bool,
    checks_url: Option<String>,
    pr_url: Option<String>,
    quick_merge_card: Option<canban_types::Card>,
    can_resume: bool,
    show_project: bool,
}

#[derive(Clone, PartialEq)]
struct PreparedStandaloneRow {
    id: String,
    title: String,
    card_dest: Option<String>,
    card_tag: Option<(u64, String)>,
    event: RowEvent,
    waiting_since: Option<u64>,
    project_name: String,
    provider_name: Option<String>,
    remote_host: Option<String>,
    context_title: String,
    needs_attn: bool,
    show_project: bool,
}

#[derive(Clone, PartialEq)]
struct PreparedUnreadCard {
    card_id: String,
    card_number: u64,
    card_title: String,
    project_name: String,
    show_project: bool,
    event: RowEvent,
    sequence: Option<u64>,
    max_seq: u64,
    unread_count: usize,
    latest_pr_action: Option<(String, String)>,
}

fn normalize_overview_row(mut row: OverviewRow) -> OverviewRow {
    if let Some(term) = row.terminal.as_mut()
        && let Some(agent) = term.agent.as_mut()
    {
        agent.lifecycle.observed_at = None;
        agent.lifecycle.last_interaction_at = None;
        agent.updated_at = 0;
    }
    row
}

fn waiting_status_pill(ui: Ui, waiting_since: Option<u64>) -> Option<AnyElement> {
    waiting_since.map(|since| {
        status(
            move || {
                let _ = ui.runtime_clock.get();
                crate::components::activity::age(since)
                    .trim_end_matches(" ago")
                    .to_owned()
            },
            Tone::Attention,
        )
        .into_element()
    })
}

fn prepare_overview_row(
    ui: Ui,
    s: &canban_types::Snapshot,
    idx: &crate::index::SnapshotIndex,
    counts: &std::collections::BTreeMap<String, canban_notifications::ReminderCount>,
    profiles: &[canban_types::Profile],
    row: OverviewRow,
    show_project: bool,
) -> PreparedOverviewRow {
    let event = describe_row(s, &row);
    let profile_tag = row.agent().map(|a| format!("@{}", a.profile.id));
    let card_host = s.card_host(&row.card.id);
    let remote_host = (card_host != "local" && !card_host.is_empty())
        .then(|| crate::hosts::host_name(ui, card_host));
    let waiting_since = row
        .agent()
        .and_then(|a| a.lifecycle.waiting.as_ref().map(|w| w.since));
    let unread = counts.get(&row.card.id).map(|c| c.unread).unwrap_or(0);
    let primary_label = primary(&row);
    let is_runtime_blocked = runtime_blocked_with_terminal(&row);
    let has_non_runtime_reason = row.reasons.iter().any(|r| r.source != "runtime");
    let has_binding = crate::terminals::model::binding(&row).is_some();
    let pr_view = crate::components::pr_status::from_card(s, idx, &row.card);
    let checks_url = pr_view.as_ref().and_then(|pv| {
        (pv.normalized_ci_state() == Some("failure"))
            .then(|| pv.checks_url())
            .flatten()
    });
    let pr_url = pr_view.as_ref().and_then(|pv| {
        (row.reasons.iter().any(|r| {
            r.message.starts_with("Changes requested on PR")
                || r.message.starts_with("Delivery PR was closed")
                || (r.message.starts_with("PR #") && r.message.contains("closed"))
        }) && !pv.pr_url.is_empty())
        .then(|| pv.pr_url.clone())
    });
    let quick_merge_card = if row.reasons.iter().any(|r| r.source == "review") {
        let project = s.projects.iter().find(|p| p.id == row.card.project_id);
        crate::card::quick_merge_eligible(&row.card, project).then(|| row.card.clone())
    } else {
        None
    };
    let can_resume = crate::terminals::model::can_resume(&row, profiles);
    PreparedOverviewRow {
        row: normalize_overview_row(row),
        event,
        profile_tag,
        remote_host,
        waiting_since,
        unread,
        primary_label,
        runtime_blocked_with_terminal: is_runtime_blocked,
        has_non_runtime_reason,
        has_binding,
        checks_url,
        pr_url,
        quick_merge_card,
        can_resume,
        show_project,
    }
}

fn prepare_standalone_row(
    ui: Ui,
    s: &canban_types::Snapshot,
    entry: &crate::terminals::Entry,
    show_project: bool,
) -> PreparedStandaloneRow {
    let id = entry.saved.id.clone();
    let is_config = s.configuration_sessions.iter().any(|c| c.id == id);
    let merge_batch = s.merge_batches.iter().find(|b| b.id == id);
    let scheduled_run = s.scheduled_runs.iter().find(|r| r.id == id);
    let needs_attn = standalone_terminal_needs_you(s, entry);
    let event = describe_standalone_terminal(entry, is_config, merge_batch, scheduled_run);
    let waiting_since = entry
        .runtime_wait()
        .map(|w| w.since)
        .filter(|&since| since > 0);
    let project_name = entry.project_name.clone();
    let context_title = if is_config {
        "Edit Project Workflows".into()
    } else if let Some(batch) = merge_batch {
        let live_cards = batch.items.iter().filter(|i| !i.evicted).count();
        format!(
            "Merge queue · {live_cards} Card(s) → {}",
            batch.target_branch
        )
    } else {
        entry.title()
    };
    let remote_host = (entry.host_id != "local" && !entry.host_id.is_empty())
        .then(|| crate::hosts::host_name(ui, &entry.host_id));
    PreparedStandaloneRow {
        id,
        title: entry.title(),
        card_dest: entry.card.as_ref().map(|c| c.id.clone()),
        card_tag: entry.card.as_ref().map(|c| (c.number, c.title.clone())),
        event,
        waiting_since,
        project_name,
        provider_name: entry.provider().map(|(_, p)| p),
        remote_host,
        context_title,
        needs_attn,
        show_project,
    }
}

fn prepare_unread_cards(
    s: &canban_types::Snapshot,
    counts: &std::collections::BTreeMap<String, canban_notifications::ReminderCount>,
    items: &[canban_notifications::Reminder],
    all_rows: &[OverviewRow],
    scope: Option<&str>,
) -> Vec<PreparedUnreadCard> {
    let show_project = scope.is_none();
    let actionable: std::collections::BTreeSet<_> = all_rows
        .iter()
        .filter(|r| r.actionable())
        .map(|r| r.card.id.clone())
        .collect();
    s.cards
        .iter()
        .filter(|c| {
            scope.is_none_or(|pid| c.project_id == pid)
                && counts.get(&c.id).is_some_and(|n| n.unread > 0)
                && !actionable.contains(&c.id)
        })
        .map(|card| {
            let latest = items
                .iter()
                .filter(|i| i.card_id == card.id && i.unread)
                .max_by_key(|i| i.sequence);
            let event = describe_unread_update(latest);
            let sequence = latest.map(|i| i.sequence);
            let max_seq = s
                .card_summaries
                .get(&card.id)
                .map(|summary| summary.last_event_seq)
                .unwrap_or(0);
            let unread_count = counts[&card.id].unread;
            let project_name = s
                .projects
                .iter()
                .find(|p| p.id == card.project_id)
                .map(|p| p.name.as_str())
                .unwrap_or("")
                .to_owned();
            let latest_pr_action = latest.and_then(|rem| {
                let meta = rem.event.as_ref()?;
                let url = meta.action_url.as_deref().filter(|u| !u.is_empty())?;
                let action_label = meta.action_label.as_deref().filter(|l| !l.is_empty())?;
                Some((url.to_owned(), action_label.to_owned()))
            });
            PreparedUnreadCard {
                card_id: card.id.clone(),
                card_number: card.number,
                card_title: card.title.clone(),
                project_name,
                show_project,
                event,
                sequence,
                max_seq,
                unread_count,
                latest_pr_action,
            }
        })
        .collect()
}

fn activity_panel(
    ui: Ui,
    project_scope: Signal<String>,
    focus_restored: Stored<bool>,
    compact_popover: bool,
) -> impl IntoElement {
    let model = computed(move || {
        let raw_scope = project_scope.get();
        let all_rows = ui.overview_rows.get();
        let entries = ui.terminal_entries.get();
        let counts = ui.reminder_counts.get();
        let items = ui.reminders.get();
        let profiles = ui.profiles.get();
        let idx = ui.index.get();
        ui.snapshot.with(|s| {
            let scope = if !raw_scope.is_empty()
                && (canban_types::is_global_project_id(&raw_scope)
                    || s.projects.iter().any(|p| p.id == raw_scope))
            {
                Some(raw_scope)
            } else {
                None
            };
            let show_project = scope.is_none();
            let mut rows: Vec<_> = all_rows
                .iter()
                .filter(|r| scope.as_deref().is_none_or(|pid| r.card.project_id == pid))
                .cloned()
                .collect();
            // Stable task/profile order rather than continuously changing activity timestamps.
            rows.sort_by(|a, b| {
                (&a.project_name, a.card.number, &a.id).cmp(&(
                    &b.project_name,
                    b.card.number,
                    &b.id,
                ))
            });

            let updates = prepare_unread_cards(s, &counts, &items, &all_rows, scope.as_deref());
            let mut pending_updates = (!updates.is_empty()).then_some(updates);
            let mut sections = Vec::new();

            for group in ["Needs you", "Running", "Queued", "Idle", "Closed"] {
                if group == "Running"
                    && let Some(u) = pending_updates.take()
                {
                    sections.push(ActivitySection::Updates(u));
                }
                let members: Vec<_> = rows
                    .iter()
                    .filter(|r| match group {
                        "Needs you" => r.actionable(),
                        "Running" => r.group == "Working" && !r.actionable(),
                        other => r.group == other && !r.actionable(),
                    })
                    .cloned()
                    .map(|r| prepare_overview_row(ui, s, &idx, &counts, &profiles, r, show_project))
                    .collect();
                let configurations: Vec<_> = if matches!(group, "Running" | "Needs you") {
                    entries
                        .iter()
                        .filter(|e| {
                            scope
                                .as_deref()
                                .is_none_or(|pid| e.project_id.as_deref() == Some(pid))
                                && if group == "Needs you" {
                                    standalone_terminal_needs_you(s, e)
                                } else {
                                    standalone_terminal_is_running(s, e)
                                }
                        })
                        .map(|e| prepare_standalone_row(ui, s, e, show_project))
                        .collect()
                } else {
                    vec![]
                };
                if members.is_empty() && configurations.is_empty() {
                    continue;
                }
                sections.push(ActivitySection::Group(ActivityGroupModel {
                    group,
                    members,
                    configurations,
                }));
            }
            if let Some(u) = pending_updates.take() {
                sections.push(ActivitySection::Updates(u));
            }
            ActivityBodyModel { sections }
        })
    });

    dynamic(move || {
        let model = model.get();
        let mut content = view()
            .max_w(px(if compact_popover { 10000. } else { 1080. }))
            .w_full()
            .gap(px(if compact_popover { 16. } else { 24. }))
            .pb_2()
            .min_w(px(0.));

        if model.sections.is_empty() {
            return content
                .child(crate::components::activity::empty(
                    "All clear",
                    "No pending gates, unread updates, or active Agent runs.",
                ))
                .into_element();
        }

        for section in model.sections {
            match section {
                ActivitySection::Updates(cards) => {
                    content = content.child(render_unread_updates_section(ui, cards));
                }
                ActivitySection::Group(g) => {
                    let count = g.members.len() + g.configurations.len();
                    let group = g.group;
                    let build_rows =
                        |members: Vec<PreparedOverviewRow>,
                         configurations: Vec<PreparedStandaloneRow>| {
                            let mut stack = view().min_w(px(0.));
                            for row in members {
                                stack = stack.child(render_prepared_overview_row(
                                    ui,
                                    row,
                                    None,
                                    focus_restored,
                                ));
                            }
                            for entry in configurations {
                                stack = stack.child(render_standalone_row(ui, entry));
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
                            build_rows(g.members, g.configurations).into_element()
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
                            sec = sec.trailing(compact_action(
                                "Terminals ↗",
                                Variant::Quiet,
                                move |_| {
                                    close(ui);
                                    ui.navigate("terminals");
                                },
                            ));
                        }
                        content = content.child(sec.child(build_rows(g.members, g.configurations)));
                    }
                }
            }
        }
        content.into_element()
    })
}

/// Production row reused by Gallery. Destination controls remain available without hover.
pub fn agent_row(ui: Ui, row: OverviewRow, selected: bool) -> impl IntoElement {
    let prepared = ui.snapshot.with_untracked(|s| {
        ui.index.with_untracked(|idx| {
            ui.reminder_counts.with_untracked(|counts| {
                ui.profiles.with_untracked(|profiles| {
                    prepare_overview_row(ui, s, idx, counts, profiles, row, true)
                })
            })
        })
    });
    render_prepared_overview_row(ui, prepared, Some(selected), stored(true))
}

pub fn describe_standalone_terminal(
    entry: &crate::terminals::Entry,
    is_config: bool,
    merge_batch: Option<&canban_types::MergeBatch>,
    scheduled_run: Option<&canban_types::ScheduledRun>,
) -> RowEvent {
    use canban_types::HerdrStatus;
    if let Some(sr) = scheduled_run
        && sr.state == "blocked"
    {
        return RowEvent {
            icon: Icon::CirclePause,
            tone: Tone::Attention,
            live: false,
            headline: "Needs decision · Scheduled Task".into(),
            detail: sr
                .summary
                .clone()
                .or_else(|| Some("Open terminal to respond to this scheduled task".into())),
        };
    }
    let herdr = entry.herdr_status();
    if herdr == HerdrStatus::Blocked {
        let (icon, tone, headline, detail) = if let Some(w) = entry.runtime_wait() {
            classify_runtime_wait(w)
        } else {
            (
                Icon::CircleHelp,
                Tone::Attention,
                "Input needed",
                Some("Waiting in terminal".into()),
            )
        };
        return RowEvent {
            icon,
            tone,
            live: false,
            headline: headline.into(),
            detail,
        };
    }
    if matches!(herdr, HerdrStatus::Error | HerdrStatus::Disconnected) || entry.needs_attention() {
        let headline = if herdr == HerdrStatus::Disconnected {
            "Terminal disconnected"
        } else if herdr == HerdrStatus::Error {
            "Terminal error"
        } else {
            "Session attention"
        };
        let detail = entry
            .attention_reason()
            .or_else(|| Some(entry.display_status()));
        return RowEvent {
            icon: Icon::TriangleAlert,
            tone: Tone::Danger,
            live: false,
            headline: headline.into(),
            detail,
        };
    }
    if is_config {
        return RowEvent {
            icon: Icon::Trigger,
            tone: Tone::Info,
            live: true,
            headline: "Configuring workflows".into(),
            detail: Some(entry.display_status()),
        };
    }
    if let Some(batch) = merge_batch {
        return RowEvent {
            icon: Icon::Branch,
            tone: Tone::Info,
            live: batch.active(),
            headline: format!("Merging batch (@{})", batch.profile_id),
            detail: Some(entry.display_status()),
        };
    }
    let headline = if herdr == HerdrStatus::Starting {
        "Starting"
    } else {
        "Working"
    };
    let branch_detail = entry
        .location
        .as_ref()
        .and_then(|l| l.branch.as_deref())
        .filter(|b| !b.is_empty())
        .map(|b| format!("Branch: {b}"));
    RowEvent {
        icon: entry.provider().map(|(ic, _)| ic).unwrap_or(Icon::Agent),
        tone: Tone::Info,
        live: true,
        headline: headline.into(),
        detail: branch_detail.or_else(|| Some(entry.title())),
    }
}

fn render_standalone_row(ui: Ui, row: PreparedStandaloneRow) -> impl IntoElement {
    let PreparedStandaloneRow {
        id,
        title,
        card_dest,
        card_tag,
        event,
        waiting_since,
        project_name,
        provider_name,
        remote_host,
        context_title,
        needs_attn,
        show_project,
    } = row;
    let id_btn = id.clone();
    let card_dest_btn = card_dest.clone();
    let top_trailing = waiting_status_pill(ui, waiting_since);

    let mut context_parts = vec![match card_tag {
        Some((number, card_title)) => {
            if card_title.is_empty() {
                format!("#{number} · {title}")
            } else {
                format!("#{number} {card_title} · {title}")
            }
        }
        None => context_title.clone(),
    }];
    if show_project && !project_name.is_empty() {
        context_parts.push(project_name.clone());
    }
    if let Some(prov_name) = provider_name {
        context_parts.push(prov_name);
    }
    let mut context_line = view()
        .flex_row()
        .w_full()
        .items_center()
        .gap_1p5()
        .min_w(px(0.))
        .overflow_hidden()
        .child(
            muted(context_parts.join(" · "))
                .flex_1()
                .min_w(px(0.))
                .truncate(),
        );
    if let Some(host_label) = remote_host {
        context_line = context_line.child(view().flex_shrink_0().child(badge(host_label)));
    }

    let summary_detail = event.detail.as_deref().unwrap_or("");
    let hint = if summary_detail.is_empty() {
        format!("{}\n{context_title} · {project_name}", event.headline)
    } else {
        format!(
            "{} · {summary_detail}\n{context_title} · {project_name}",
            event.headline
        )
    };

    view()
        .flex_row()
        .items_center()
        .gap_2()
        .py_0p5()
        .min_w(px(0.))
        .border_b_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .tooltip(hint_tooltip(hint))
                .child(
                    canban_ui_kit::press_target()
                        .w_full()
                        .min_w(px(0.))
                        .items_center()
                        .flex_row()
                        .gap_2p5()
                        .py_2()
                        .px_2()
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
                        .child(tone_icon_badge(event.icon, event.tone, event.live))
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.))
                                .gap_1()
                                .child(event_headline_row(
                                    event.headline,
                                    event.tone,
                                    event.detail,
                                    top_trailing,
                                ))
                                .child(context_line),
                        ),
                ),
        )
        .child(compact_action(
            if card_dest_btn.is_some() {
                "Card & Terminal →"
            } else {
                "Terminal →"
            },
            if needs_attn {
                Variant::Secondary
            } else {
                Variant::Quiet
            },
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

fn render_prepared_overview_row(
    ui: Ui,
    prepared: PreparedOverviewRow,
    selected_override: Option<bool>,
    focus_restored: Stored<bool>,
) -> impl IntoElement {
    let PreparedOverviewRow {
        row,
        event,
        profile_tag,
        remote_host,
        waiting_since,
        unread,
        primary_label,
        runtime_blocked_with_terminal: is_runtime_blocked,
        has_non_runtime_reason,
        has_binding,
        checks_url,
        pr_url,
        quick_merge_card,
        can_resume,
        show_project,
    } = prepared;
    let reference = element_ref();
    let row_id_bg = row.id.clone();
    let row_id_focus = row.id.clone();
    let activate_row = row.clone();

    let event_summary = match &event.detail {
        Some(d) if !d.is_empty() => format!("{} · {d}", event.headline),
        _ => event.headline.clone(),
    };
    let metadata_str = match &profile_tag {
        Some(p) => format!("{} · {p}", row.project_name),
        None => row.project_name.clone(),
    };
    let hint = format!(
        "{event_summary}\n#{} {}\n{metadata_str}",
        row.card.number, row.card.title
    );

    let top_trailing = waiting_status_pill(ui, waiting_since);
    let mut context_pills = Vec::new();
    if let Some(host_label) = remote_host {
        context_pills.push(badge(host_label).into_element());
    }
    if unread > 0 && row.actionable() && top_trailing.is_none() {
        context_pills.push(badge(format!("{unread} unread")).into_element());
    }

    let main = canban_ui_kit::press_target()
        .element_ref(&reference)
        .w_full()
        .min_w(px(0.))
        .items_center()
        .flex_row()
        .gap_2p5()
        .py_2()
        .px_2()
        .rounded(px(RADIUS))
        .bg(CLEAR)
        .hover(|s| s.bg(CARD_HOVER))
        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
        .accessibility_label(format!(
            "{event_summary} · #{} {} · {metadata_str} · {}",
            row.card.number, row.card.title, primary_label
        ))
        .child(tone_icon_badge(event.icon, event.tone, event.live))
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .gap_1()
                .child(event_headline_row(
                    event.headline,
                    event.tone,
                    event.detail,
                    top_trailing,
                ))
                .child(card_context_row(
                    row.card.number,
                    row.card.title.clone(),
                    show_project.then(|| row.project_name.clone()),
                    profile_tag,
                    context_pills,
                )),
        );

    let button_row = row.clone();
    let mut actions = view().flex_row().items_center().gap_1p5().flex_shrink_0();

    if let Some(ci_url) = checks_url {
        actions = actions.child(compact_action("Checks ↗", Variant::Quiet, move |_| {
            close(ui);
            crate::github::open_url(ui, ci_url.clone());
        }));
    } else if let Some(pr_url) = pr_url {
        actions = actions.child(compact_action("PR ↗", Variant::Quiet, move |_| {
            close(ui);
            crate::github::open_url(ui, pr_url.clone());
        }));
    }

    if let Some(merge_card) = quick_merge_card {
        actions = actions.child(
            compact_action("Merge", Variant::Quiet, move |_| {
                close(ui);
                crate::card::quick_merge_start(ui, merge_card.clone());
            })
            .disabled(move || crate::card::quick_merge_busy(ui)),
        );
    }
    if can_resume {
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
    if !is_runtime_blocked && has_non_runtime_reason && has_binding {
        let id = row.id.clone();
        let cid = row.card.id.clone();
        actions = actions.child(compact_action("Terminal", Variant::Quiet, move |_| {
            ui.overview_selection.set(id.clone());
            close(ui);
            ui.open_card_with_terminal(cid.clone(), Some(id.clone()));
        }));
    }
    let primary_variant = if row.actionable() {
        Variant::Secondary
    } else {
        Variant::Quiet
    };
    actions = actions.child(compact_action(primary_label, primary_variant, move |_| {
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
        .bg(move || {
            let is_selected = selected_override
                .unwrap_or_else(|| ui.overview_selection.with(|s| s == &row_id_bg));
            if is_selected { SELECTED } else { CLEAR }
        })
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .on_layout(move |_: &Bounds<Points>| {
                    let is_selected = selected_override.unwrap_or_else(|| {
                        ui.overview_selection.with_untracked(|s| s == &row_id_focus)
                    });
                    if is_selected && !focus_restored.replace(true) {
                        reference.focus();
                    }
                })
                .tooltip(hint_tooltip(hint))
                .child(main.on_press(move |_: &()| activate(ui, &activate_row))),
        )
        .child(actions)
}

fn render_unread_updates_section(ui: Ui, cards: Vec<PreparedUnreadCard>) -> SectionGroup {
    let count = cards.len();
    let mut rows = view().min_w(px(0.));
    for card in cards {
        let id = card.card_id.clone();
        let id_btn = id.clone();
        let id_read = id.clone();
        let event = card.event;
        let sequence = card.sequence;
        let max_seq = card.max_seq;
        let unread_count = card.unread_count;
        let project = card.project_name;

        let event_summary = match &event.detail {
            Some(d) if !d.is_empty() => format!("{} · {d}", event.headline),
            _ => event.headline.clone(),
        };
        let hint = format!(
            "{event_summary}\n#{} {}\n{project}",
            card.card_number, card.card_title
        );
        let extra_pill =
            (unread_count > 1).then(|| badge(format!("+{}", unread_count - 1)).into_element());

        let mut row_view = view()
            .flex_row()
            .items_center()
            .gap_1p5()
            .py_0p5()
            .min_w(px(0.))
            .border_b_1()
            .border_color(BORDER)
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .tooltip(hint_tooltip(hint))
                    .child(
                        canban_ui_kit::press_target()
                            .w_full()
                            .min_w(px(0.))
                            .items_center()
                            .flex_row()
                            .gap_2p5()
                            .py_2()
                            .px_2()
                            .rounded(px(RADIUS))
                            .bg(CLEAR)
                            .hover(|s| s.bg(CARD_HOVER))
                            .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                            .accessibility_label(format!(
                                "{event_summary} · #{} {} · {project} · Card →",
                                card.card_number, card.card_title
                            ))
                            .on_press(move |_: &()| {
                                close(ui);
                                ui.open_card(id.clone());
                                ui.card_event.set(sequence);
                            })
                            .child(tone_icon_badge(event.icon, event.tone, false))
                            .child(
                                view()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .gap_1()
                                    .child(event_headline_row(
                                        event.headline,
                                        event.tone,
                                        event.detail,
                                        extra_pill,
                                    ))
                                    .child(card_context_row(
                                        card.card_number,
                                        card.card_title,
                                        card.show_project.then_some(project),
                                        None,
                                        Vec::new(),
                                    )),
                            ),
                    ),
            );
        if let Some((action_url, action_label)) = card.latest_pr_action {
            row_view = row_view.child(compact_action(action_label, Variant::Quiet, move |_| {
                close(ui);
                crate::github::open_url(ui, action_url.clone());
            }));
        }
        rows = rows.child(
            row_view
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

    section_group("Updates")
        .count(count)
        .tone(Tone::Info)
        .child(rows)
}
