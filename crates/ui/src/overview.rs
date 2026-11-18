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
                .events
                .iter()
                .filter(|e| e.card_id == cid)
                .map(|e| e.sequence)
                .max()
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
    if crate::APP.get().is_none() {
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
    if crate::APP.get().is_none() {
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

fn standalone_terminal_needs_you(s: &canban_types::Snapshot, e: &crate::terminals::Entry) -> bool {
    e.agent.is_none()
        && e.listed()
        && (e.needs_attention()
            || e.herdr_status().is_attention()
            || (!e.ended()
                && s.scheduled_runs
                    .iter()
                    .any(|r| r.id == e.saved.id && r.state == "blocked")))
}

fn standalone_terminal_is_running(s: &canban_types::Snapshot, e: &crate::terminals::Entry) -> bool {
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

fn running_agents_count_from(
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
                        .child(scroll().max_h(px(560.)).p_3p5().child(dynamic(move || {
                            activity_body(ui, project_scope.get(), stored(true), true)
                        }))),
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
struct RowEvent {
    icon: Icon,
    tone: Tone,
    live: bool,
    headline: String,
    detail: Option<String>,
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
    match reason.source.as_str() {
        "runtime" => {
            let runtime = row.agent().map(|a| a.runtime_status());
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

fn reason_priority(source: &str, row: &OverviewRow) -> u8 {
    match source {
        "runtime" => {
            if row.agent().map(|a| a.runtime_status()) == Some("blocked") {
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

fn describe_row(snapshot: &canban_types::Snapshot, row: &OverviewRow) -> RowEvent {
    if !row.reasons.is_empty() {
        let mut ordered: Vec<_> = row.reasons.iter().collect();
        ordered.sort_by_key(|r| reason_priority(&r.source, row));
        let (icon, tone, headline, mut detail) = classify_reason(snapshot, row, ordered[0]);
        if ordered.len() > 1 {
            let extras: Vec<_> = ordered[1..]
                .iter()
                .map(|r| {
                    let (_, _, h, d) = classify_reason(snapshot, row, r);
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

fn describe_unread_update(
    snapshot: &canban_types::Snapshot,
    card_id: &str,
    latest: Option<&canban_notifications::Reminder>,
) -> RowEvent {
    let raw_msg = latest.map(|i| i.message.as_str()).unwrap_or("");
    let matched_event = latest.and_then(|rem| {
        snapshot
            .events
            .iter()
            .filter(|e| e.card_id == card_id && e.sequence == rem.sequence)
            .max_by_key(|e| e.created_at)
    });
    let agent_tag = matched_event
        .and_then(|e| e.actor.strip_prefix("agent:"))
        .map(|a| format!("@{a}"));

    if let Some(ev) = matched_event {
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
            let state = ev.data["state"].as_str().unwrap_or("completed");
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
    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(dynamic(move || {
            let s = ui.snapshot.get();
            let all_rows = ui.overview_rows.get();
            let entries = ui.terminal_entries.get();
            let scope = project_scope.get();
            let valid_scope = if !scope.is_empty()
                && !canban_types::is_global_project_id(&scope)
                && !s.projects.iter().any(|p| p.id == scope)
            {
                String::new()
            } else {
                scope
            };
            let scope_opt = (!valid_scope.is_empty()).then_some(valid_scope.as_str());
            let actionable_count = all_rows
                .iter()
                .filter(|r| scope_opt.is_none_or(|pid| r.card.project_id == pid) && r.actionable())
                .count()
                + entries
                    .iter()
                    .filter(|e| {
                        scope_opt.is_none_or(|pid| e.project_id.as_deref() == Some(pid))
                            && standalone_terminal_needs_you(&s, e)
                    })
                    .count();
            let running_count = running_agents_count_from(&s, &all_rows, &entries, scope_opt);
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

            let has_global = entries.iter().any(|e| {
                e.listed()
                    && e.project_id
                        .as_deref()
                        .is_some_and(canban_types::is_global_project_id)
            }) || s.scheduled_runs.iter().any(|r| {
                canban_types::is_global_project_id(&r.project_id)
                    && (r.active() || r.state == "blocked")
            });
            if s.projects.len() > 1 || has_global {
                let all_selected = valid_scope.is_empty();
                let mut filter_seg = segmented().child(compact_segment(
                    "All",
                    None,
                    move || all_selected,
                    move |_| project_scope.set(String::new()),
                ));
                if has_global {
                    let global_selected = canban_types::is_global_project_id(&valid_scope);
                    filter_seg = filter_seg.child(compact_segment(
                        "Global",
                        None,
                        move || global_selected,
                        move |_| project_scope.set(canban_types::GLOBAL_PROJECT_ID.into()),
                    ));
                }
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
                    scope
                        .as_deref()
                        .is_none_or(|pid| e.project_id.as_deref() == Some(pid))
                        && if group == "Needs you" {
                            standalone_terminal_needs_you(&s, e)
                        } else {
                            standalone_terminal_is_running(&s, e)
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

        let show_project = scope.is_none();
        let build_rows = |members: Vec<OverviewRow>,
                          configurations: Vec<crate::terminals::Entry>| {
            let mut stack = view().min_w(px(0.));
            for row in members {
                let selected = ui.overview_selection.get() == row.id;
                stack = stack.child(row_view(ui, row, selected, focus_restored, show_project));
            }
            for entry in configurations {
                stack = stack.child(config_session_row(ui, entry, show_project));
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
    row_view(ui, row, selected, stored(true), true)
}

fn describe_standalone_terminal(
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

fn config_session_row(
    ui: Ui,
    entry: crate::terminals::Entry,
    show_project: bool,
) -> impl IntoElement {
    let id = entry.saved.id.clone();
    let id_btn = id.clone();
    let (is_config, merge_batch, scheduled_run, needs_attn) = ui.snapshot.with_untracked(|s| {
        (
            s.configuration_sessions.iter().any(|c| c.id == id),
            s.merge_batches.iter().find(|b| b.id == id).cloned(),
            s.scheduled_runs.iter().find(|r| r.id == id).cloned(),
            standalone_terminal_needs_you(s, &entry),
        )
    });
    let event = describe_standalone_terminal(
        &entry,
        is_config,
        merge_batch.as_ref(),
        scheduled_run.as_ref(),
    );
    let waiting_age = entry
        .runtime_wait()
        .map(|w| w.since)
        .filter(|&s| s > 0)
        .map(|since| {
            crate::components::activity::age(since)
                .trim_end_matches(" ago")
                .to_owned()
        });
    let top_trailing = waiting_age.map(|wait| status(wait, Tone::Attention).into_element());
    let project_name = entry.project_name.clone();
    let context_title = if is_config {
        "Edit Project Workflows".into()
    } else if let Some(ref batch) = merge_batch {
        let live_cards = batch.items.iter().filter(|i| !i.evicted).count();
        format!(
            "Merge queue · {live_cards} Card(s) → {}",
            batch.target_branch
        )
    } else {
        entry.title()
    };
    let card_dest = entry.card.as_ref().map(|c| c.id.clone());
    let card_dest_btn = card_dest.clone();

    let mut context_parts = vec![match &entry.card {
        Some(c) => {
            if c.title.is_empty() {
                format!("#{} · {}", c.number, entry.title())
            } else {
                format!("#{} {} · {}", c.number, c.title, entry.title())
            }
        }
        None => context_title.clone(),
    }];
    if show_project && !project_name.is_empty() {
        context_parts.push(project_name.clone());
    }
    if let Some((_, prov_name)) = entry.provider() {
        context_parts.push(prov_name);
    }
    let remote_host = (entry.host_id != "local" && !entry.host_id.is_empty())
        .then(|| crate::hosts::host_name(ui, &entry.host_id));
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

fn row_view(
    ui: Ui,
    row: OverviewRow,
    selected: bool,
    focus_restored: Stored<bool>,
    show_project: bool,
) -> impl IntoElement {
    let reference = element_ref();
    let activate_row = row.clone();
    let event = ui.snapshot.with(|s| describe_row(s, &row));
    let profile_tag = row.agent().map(|a| format!("@{}", a.profile.id));
    let card_host = ui.snapshot.with(|s| s.card_host(&row.card.id).to_owned());
    let remote_host = (card_host != "local" && !card_host.is_empty())
        .then(|| crate::hosts::host_name(ui, &card_host));
    let waiting_age = row
        .agent()
        .and_then(|a| a.lifecycle.waiting.as_ref().map(|w| w.since))
        .map(|since| {
            crate::components::activity::age(since)
                .trim_end_matches(" ago")
                .to_owned()
        });
    let unread = ui
        .reminder_counts
        .with(|m| m.get(&row.card.id).map(|c| c.unread).unwrap_or(0));

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

    let top_trailing = waiting_age.map(|wait| status(wait, Tone::Attention).into_element());
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
            row.card.number,
            row.card.title,
            primary(&row)
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
    if !runtime_blocked_with_terminal(&row)
        && row.reasons.iter().any(|r| r.source != "runtime")
        && crate::terminals::model::binding(&row).is_some()
    {
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
    let show_project = scope.is_none();
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
        let event = describe_unread_update(&s, &id, latest);
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

        let event_summary = match &event.detail {
            Some(d) if !d.is_empty() => format!("{} · {d}", event.headline),
            _ => event.headline.clone(),
        };
        let hint = format!(
            "{event_summary}\n#{} {}\n{project}",
            card.number, card.title
        );
        let extra_pill =
            (unread_count > 1).then(|| badge(format!("+{}", unread_count - 1)).into_element());

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
                                    card.number, card.title
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
                                            card.number,
                                            card.title.clone(),
                                            show_project.then_some(project),
                                            None,
                                            Vec::new(),
                                        )),
                                ),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn base_card() -> canban_types::Card {
        serde_json::from_value(json!({
            "id": "c1",
            "project_id": "p1",
            "number": 46,
            "title": "优化提醒的条目",
            "body": "",
            "stage": "develop",
            "revision": 1,
            "created_at": 1
        }))
        .unwrap()
    }

    #[test]
    fn describe_row_promotes_permission_and_cleans_redundant_prefixes() {
        let mut s = canban_types::Snapshot::default();
        let card = base_card();
        let mut attempt: canban_types::AgentRun = serde_json::from_value(json!({
            "project_id": "p1",
            "card_id": "c1",
            "number": 1,
            "stage": "develop",
            "trigger": "stage_entry",
            "profile": canban_types::Config::default().profiles[0],
            "instruction": "task",
            "state": "running",
            "terminal": "t1",
            "cwd": "/tmp",
            "input_path": "in",
            "connection_state": "connected",
            "observed": {
                "state": "live",
                "status": {
                    "state": "blocked",
                    "kind": "permission",
                    "title": "Allow command?",
                    "msg": "cargo test --workspace"
                }
            },
            "updated_at": 1
        }))
        .unwrap();
        attempt.lifecycle.observed_at = Some(u64::MAX);
        attempt.lifecycle.waiting = Some(canban_types::RuntimeWait {
            id: "w1".into(),
            kind: "permission".into(),
            title: "Allow command?".into(),
            message: "cargo test --workspace".into(),
            since: 1,
        });
        s.cards.push(card);
        s.terminals.push(canban_types::TerminalEntry {
            id: "c1:codex".into(),
            conversation_id: Some("c1:codex".into()),
            number: 1,
            name: None,
            origin_project_id: None,
            origin_card_id: Some("c1".into()),
            agent_engine: None,
            runtime: None,
            agent: Some(attempt),
            created_at: 1,
        });

        let rows = overview(&s);
        let ev = describe_row(&s, &rows[0]);
        assert_eq!(ev.icon, Icon::ShieldAlert);
        assert_eq!(ev.tone, Tone::Attention);
        assert_eq!(ev.headline, "Permission required");
        assert_eq!(
            ev.detail.as_deref(),
            Some("Allow command? · cargo test --workspace")
        );
    }

    #[test]
    fn describe_row_distinguishes_review_merge_conflict_and_failure_states() {
        let mut s = canban_types::Snapshot::default();
        let mut card = base_card();
        card.stage = "review".into();
        let mut att = canban_scheduler::agent_run(
            "p1",
            "c1",
            1,
            "review",
            "stage_entry",
            canban_types::Config::default().profiles[0].clone(),
            "review".into(),
            None,
        );
        att.state = "completed".into();
        let tid = "c1:codex".to_owned();
        card.human_gate = Some(canban_types::HumanGate {
            reason: "Agent report ready; human review required".into(),
            terminal_id: Some(tid.clone()),
            next_stage: None,
        });
        s.reports.push(canban_types::Report {
            id: "rep-1".into(),
            terminal_id: Some(tid.clone()),
            workflow_run_id: None,
            actor: "agent:developer".into(),
            project_id: "p1".into(),
            card_id: "c1".into(),
            state: "completed".into(),
            message: "Implemented event-first hierarchy and semantic icons".into(),
            path: None,
            request_key: "k1".into(),
            fingerprint: "f1".into(),
            created_at: 2,
        });
        s.cards.push(card);
        s.terminals.push(canban_types::TerminalEntry {
            id: tid.clone(),
            conversation_id: Some(tid),
            number: 1,
            name: None,
            origin_project_id: None,
            origin_card_id: Some("c1".into()),
            agent_engine: None,
            runtime: None,
            agent: Some(att),
            created_at: 1,
        });

        let rows = overview(&s);
        let ev = describe_row(&s, &rows[0]);
        assert_eq!(ev.icon, Icon::Eye);
        assert_eq!(ev.tone, Tone::Attention);
        assert_eq!(ev.headline, "Ready for review");
        assert_eq!(
            ev.detail.as_deref(),
            Some("Implemented event-first hierarchy and semantic icons")
        );

        // Merge conflict gate surfaces GitMerge icon + Merge conflict headline.
        s.cards[0].human_gate.as_mut().unwrap().reason =
            "Merge conflict with main after 2 attempts".into();
        let rows = overview(&s);
        let merge_ev = describe_row(&s, &rows[0]);
        assert_eq!(merge_ev.icon, Icon::GitMerge);
        assert_eq!(merge_ev.headline, "Merge conflict");
        assert_eq!(
            merge_ev.detail.as_deref(),
            Some("Merge conflict with main after 2 attempts")
        );

        // Failed Agent run gate surfaces Danger TriangleAlert + Agent failed.
        s.terminals[0].agent.as_mut().unwrap().state = "failed".into();
        s.cards[0].human_gate.as_mut().unwrap().reason =
            "failed: Build failed due to missing symbol".into();
        let rows = overview(&s);
        let fail_ev = describe_row(&s, &rows[0]);
        assert_eq!(fail_ev.icon, Icon::TriangleAlert);
        assert_eq!(fail_ev.tone, Tone::Danger);
        assert_eq!(fail_ev.headline, "Agent failed");
        assert_eq!(
            fail_ev.detail.as_deref(),
            Some("Build failed due to missing symbol")
        );
    }

    #[test]
    fn describe_unread_update_strips_machine_prefixes_and_maps_semantic_icons() {
        let mut s = canban_types::Snapshot::default();
        s.cards.push(base_card());
        s.events.push(canban_types::Event {
            id: "e1".into(),
            project_id: "p1".into(),
            card_id: "c1".into(),
            sequence: 1,
            kind: "comment.replied".into(),
            actor: "agent:agy".into(),
            message: "已整理好 ASCII 方案，请确认".into(),
            data: json!({"question_id": "q1"}),
            created_at: 1,
        });
        let items = canban_notifications::reminders(&s);
        let ev = describe_unread_update(&s, "c1", items.first());
        assert_eq!(ev.icon, Icon::MessageSquare);
        assert_eq!(ev.tone, Tone::Info);
        assert_eq!(ev.headline, "@agy replied");
        assert_eq!(ev.detail.as_deref(), Some("已整理好 ASCII 方案，请确认"));

        // Stage completed report strips `completed: ` prefix.
        s.events.push(canban_types::Event {
            id: "e2".into(),
            project_id: "p1".into(),
            card_id: "c1".into(),
            sequence: 2,
            kind: "execution.reported".into(),
            actor: "agent:developer".into(),
            message: "completed: All unit tests passed".into(),
            data: json!({"report_id": "r2", "state": "completed"}),
            created_at: 2,
        });
        let items = canban_notifications::reminders(&s);
        let latest = items.iter().max_by_key(|i| i.sequence);
        let done_ev = describe_unread_update(&s, "c1", latest);
        assert_eq!(done_ev.icon, Icon::Check);
        assert_eq!(done_ev.tone, Tone::Success);
        assert_eq!(done_ev.headline, "Stage completed · @developer");
        assert_eq!(done_ev.detail.as_deref(), Some("All unit tests passed"));

        // Returned to Backlog strips `Returned to Backlog: ` prefix.
        s.events.push(canban_types::Event {
            id: "e3".into(),
            project_id: "p1".into(),
            card_id: "c1".into(),
            sequence: 3,
            kind: "execution.reported".into(),
            actor: "agent:developer".into(),
            message: "Returned to Backlog: Already implemented in main".into(),
            data: json!({"report_id": "r3", "state": "backlog"}),
            created_at: 3,
        });
        let items = canban_notifications::reminders(&s);
        let latest = items.iter().max_by_key(|i| i.sequence);
        let backlog_ev = describe_unread_update(&s, "c1", latest);
        assert_eq!(backlog_ev.icon, Icon::Undo);
        assert_eq!(backlog_ev.tone, Tone::Neutral);
        assert_eq!(backlog_ev.headline, "Returned to Backlog");
        assert_eq!(
            backlog_ev.detail.as_deref(),
            Some("Already implemented in main")
        );
    }

    #[test]
    fn describe_row_suppresses_automatic_unreported_idle_until_one_hour_fallback() {
        let mut s = canban_types::Snapshot::default();
        s.cards.push(base_card());
        let mut attempt = canban_scheduler::agent_run(
            "p1",
            "c1",
            1,
            "develop",
            "stage_entry",
            canban_types::Config::default().profiles[0].clone(),
            "implement".into(),
            None,
        );
        attempt.state = "running".into();
        attempt.terminal = Some("t1".into());
        attempt.connection_state = "connected".into();
        attempt.observed = Some(json!({"state": "live", "turn": 1, "status": {"state": "idle"}}));
        attempt.lifecycle.observed_at = Some(u64::MAX);
        attempt.lifecycle.idle_since = Some(canban_types::now().saturating_sub(5 * 60_000));
        s.terminals.push(canban_types::TerminalEntry {
            id: "c1:agy".into(),
            conversation_id: Some("c1:agy".into()),
            number: 1,
            name: None,
            origin_project_id: None,
            origin_card_id: Some("c1".into()),
            agent_engine: None,
            runtime: None,
            agent: Some(attempt),
            created_at: 1,
        });

        // Under 1 hour (e.g. agy running background tasks), no "Stopped before reporting" error.
        let rows = overview(&s);
        assert_eq!(rows[0].group, "Idle");
        assert!(!rows[0].actionable());
        let quiet_ev = describe_row(&s, &rows[0]);
        assert_eq!(quiet_ev.tone, Tone::Neutral);
        assert_eq!(quiet_ev.headline, "Idle");

        // After 1 continuous hour without reporting, surfaces "Stopped before reporting".
        s.terminals[0].agent.as_mut().unwrap().lifecycle.idle_since = Some(
            canban_types::now()
                .saturating_sub(canban_notifications::UNREPORTED_IDLE_ALERT_MS + 1_000),
        );
        let rows = overview(&s);
        assert_eq!(rows[0].group, "Needs attention");
        assert!(rows[0].actionable());
        let stalled_ev = describe_row(&s, &rows[0]);
        assert_eq!(stalled_ev.icon, Icon::TriangleAlert);
        assert_eq!(stalled_ev.tone, Tone::Danger);
        assert_eq!(stalled_ev.headline, "Stopped before reporting");
        assert_eq!(stalled_ev.detail.as_deref(), Some("Inspect terminal"));
    }

    #[test]
    fn standalone_and_manual_shell_agent_terminals_route_into_needs_you_and_running() {
        let mut s = canban_types::Snapshot::default();
        s.projects.push(
            serde_json::from_value(json!({
                "id": "p1",
                "name": "Canban",
                "directory": "/tmp/canban",
                "workflow": canban_types::Workflow::default(),
                "created_at": 1
            }))
            .unwrap(),
        );
        // 1. Plain shell in zsh -> neither Needs you nor Running.
        s.terminals.push(canban_types::TerminalEntry {
            id: "shell-plain".into(),
            conversation_id: None,
            number: 1,
            name: Some("Terminal 1".into()),
            origin_project_id: Some("p1".into()),
            origin_card_id: None,
            agent_engine: None,
            runtime: Some(canban_types::TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-1".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: 1,
        });
        // 2. Manual shell running `claude` in `working` -> enters Running, not Needs you, and consumes 0 Scheduler slots!
        s.terminals.push(canban_types::TerminalEntry {
            id: "shell-working-claude".into(),
            conversation_id: None,
            number: 2,
            name: Some("Terminal 2".into()),
            origin_project_id: Some("p1".into()),
            origin_card_id: None,
            agent_engine: None,
            runtime: Some(canban_types::TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-2".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: Some("working".into()),
                agent_turn: Some(1),
                observed_agent: Some("claude".into()),
                waiting: None,
            }),
            agent: None,
            created_at: 2,
        });
        // 3. Manual shell running `agy` blocked on permission -> enters Needs you with Permission required!
        s.terminals.push(canban_types::TerminalEntry {
            id: "shell-blocked-agy".into(),
            conversation_id: None,
            number: 3,
            name: Some("Terminal 3".into()),
            origin_project_id: Some("p1".into()),
            origin_card_id: None,
            agent_engine: None,
            runtime: Some(canban_types::TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-3".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: Some("blocked".into()),
                agent_turn: Some(1),
                observed_agent: Some("agy".into()),
                waiting: Some(canban_types::RuntimeWait {
                    id: "w-perm".into(),
                    kind: "permission".into(),
                    title: "Allow command?".into(),
                    message: "git status".into(),
                    since: 10,
                }),
            }),
            agent: None,
            created_at: 3,
        });

        assert_eq!(canban_scheduler::active_count(&s), 0);
        let entries = crate::terminals::entries(&s);
        assert!(!standalone_terminal_needs_you(&s, &entries[0]));
        assert!(!standalone_terminal_is_running(&s, &entries[0]));

        assert!(!standalone_terminal_needs_you(&s, &entries[1]));
        assert!(standalone_terminal_is_running(&s, &entries[1]));
        let working_ev = describe_standalone_terminal(&entries[1], false, None, None);
        assert_eq!(working_ev.headline, "Working");
        assert!(working_ev.live);

        assert!(standalone_terminal_needs_you(&s, &entries[2]));
        assert!(!standalone_terminal_is_running(&s, &entries[2]));
        let blocked_ev = describe_standalone_terminal(&entries[2], false, None, None);
        assert_eq!(blocked_ev.icon, Icon::ShieldAlert);
        assert_eq!(blocked_ev.tone, Tone::Attention);
        assert_eq!(blocked_ev.headline, "Permission required");
        assert_eq!(
            blocked_ev.detail.as_deref(),
            Some("Allow command? · git status")
        );
    }

    #[test]
    fn trigger_pill_and_popover_count_all_running_agents_without_slot_denominator() {
        use nsh_core::{ApproxTextEngine, Core, MountItem, geometry::size};

        fn collect_mounted_texts(batch: nsh_core::MountBatch) -> Vec<String> {
            batch
                .items
                .into_iter()
                .filter_map(|item| match item {
                    MountItem::Text { text, .. } => Some(text.text.to_string()),
                    _ => None,
                })
                .collect()
        }

        #[component]
        fn ProbeRunningCount() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut s = canban_types::Snapshot::default();
            s.projects.push(
                serde_json::from_value(json!({
                    "id": "p1",
                    "name": "Canban",
                    "directory": "/tmp/canban",
                    "workflow": canban_types::Workflow::default(),
                    "created_at": 1
                }))
                .unwrap(),
            );
            s.cards.push(base_card());

            // 1. Managed Card Agent actively working (1 Scheduler slot).
            let mut managed = canban_scheduler::agent_run(
                "p1",
                "c1",
                1,
                "develop",
                "stage_entry",
                canban_types::Config::default().profiles[0].clone(),
                "implement".into(),
                None,
            );
            managed.state = "running".into();
            managed.terminal = Some("pty-card".into());
            managed.connection_state = "connected".into();
            managed.observed =
                Some(json!({"state": "live", "turn": 1, "status": {"state": "working"}}));
            managed.lifecycle.observed_at = Some(u64::MAX);
            s.terminals.push(canban_types::TerminalEntry {
                id: "c1:developer".into(),
                conversation_id: Some("c1:developer".into()),
                number: 1,
                name: None,
                origin_project_id: None,
                origin_card_id: Some("c1".into()),
                agent_engine: None,
                runtime: None,
                agent: Some(managed),
                created_at: 1,
            });

            // 2. Manual shell running `claude` (0 Scheduler slots, 1 running Agent).
            s.terminals.push(canban_types::TerminalEntry {
                id: "manual-claude".into(),
                conversation_id: None,
                number: 2,
                name: Some("Terminal 2".into()),
                origin_project_id: Some("p1".into()),
                origin_card_id: None,
                agent_engine: None,
                runtime: Some(canban_types::TerminalRuntime {
                    machine: "local".into(),
                    owner_core_id: None,
                    core_id: Some("core".into()),
                    terminal: Some("pty-2".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: Some("working".into()),
                    agent_turn: Some(1),
                    observed_agent: Some("claude".into()),
                    waiting: None,
                }),
                agent: None,
                created_at: 2,
            });

            // 3. Ephemeral CLI Agent launched by user (0 Scheduler slots, 1 running Agent).
            s.terminals.push(canban_types::TerminalEntry {
                id: "ephemeral-agy".into(),
                conversation_id: None,
                number: 3,
                name: Some("Antigravity 1".into()),
                origin_project_id: Some("p1".into()),
                origin_card_id: None,
                agent_engine: Some("agy".into()),
                runtime: Some(canban_types::TerminalRuntime {
                    machine: "local".into(),
                    owner_core_id: None,
                    core_id: Some("core".into()),
                    terminal: Some("pty-3".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: Some("working".into()),
                    agent_turn: Some(1),
                    observed_agent: Some("agy".into()),
                    waiting: None,
                }),
                agent: None,
                created_at: 3,
            });

            // 4. Plain interactive shell (neither Scheduler slot nor running Agent).
            s.terminals.push(canban_types::TerminalEntry {
                id: "plain-shell".into(),
                conversation_id: None,
                number: 4,
                name: Some("Terminal 3".into()),
                origin_project_id: Some("p1".into()),
                origin_card_id: None,
                agent_engine: None,
                runtime: Some(canban_types::TerminalRuntime {
                    machine: "local".into(),
                    owner_core_id: None,
                    core_id: Some("core".into()),
                    terminal: Some("pty-4".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                }),
                agent: None,
                created_at: 4,
            });

            assert_eq!(canban_scheduler::active_count(&s), 1);
            ui.max_agents.set(8);
            ui.snapshot.set(s);
            ui.utility_popup.set("notifications".into());

            assert_eq!(attention_summary(ui), (0, 0, 3));

            view()
                .w(px(1200.))
                .h(px(900.))
                .child(trigger_pill(ui))
                .child(popover(ui))
                .child(screen(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(900.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbeRunningCount>());
        let texts = collect_mounted_texts(core.commit().expect("mount batch"));

        assert!(
            texts.iter().any(|t| t == "3"),
            "expected trigger pill running count `3` (1 managed + 2 user-launched), got {texts:?}"
        );
        let running_badges = texts.iter().filter(|t| *t == "3 running").count();
        assert!(
            running_badges >= 2,
            "expected `3 running` status badge in both popover and overview screen, got {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("/8") || t.contains("/ 8")),
            "did not expect `/8` Scheduler slot denominator in Activity & Notifications, got {texts:?}"
        );
    }

    #[test]
    fn read_all_and_close_keep_stable_node_ids_across_snapshot_updates() {
        use nsh_core::{
            ApproxTextEngine, Core, EventKind, EventPayload, MountItem, NodeId, ROOT_ID,
            geometry::size,
        };

        fn snapshot_with_unread(unread_msg: &str, running: bool) -> canban_types::Snapshot {
            let mut s = canban_types::Snapshot::default();
            s.projects.push(
                serde_json::from_value(json!({
                    "id": "p1",
                    "name": "Canban",
                    "directory": "/tmp/canban",
                    "workflow": canban_types::Workflow::default(),
                    "created_at": 1
                }))
                .unwrap(),
            );
            s.cards.push(base_card());
            s.events.push(canban_types::Event {
                id: "e1".into(),
                project_id: "p1".into(),
                card_id: "c1".into(),
                sequence: 1,
                kind: "comment.replied".into(),
                actor: "agent:agy".into(),
                message: unread_msg.into(),
                data: json!({"question_id": "q1"}),
                created_at: 10,
            });
            if running {
                let mut managed = canban_scheduler::agent_run(
                    "p1",
                    "c1",
                    1,
                    "develop",
                    "stage_entry",
                    canban_types::Config::default().profiles[0].clone(),
                    "implement".into(),
                    None,
                );
                managed.state = "running".into();
                managed.terminal = Some("pty-card".into());
                managed.connection_state = "connected".into();
                managed.observed =
                    Some(json!({"state": "live", "turn": 1, "status": {"state": "working"}}));
                managed.lifecycle.observed_at = Some(u64::MAX);
                s.terminals.push(canban_types::TerminalEntry {
                    id: "c1:developer".into(),
                    conversation_id: Some("c1:developer".into()),
                    number: 1,
                    name: None,
                    origin_project_id: None,
                    origin_card_id: Some("c1".into()),
                    agent_engine: None,
                    runtime: None,
                    agent: Some(managed),
                    created_at: 1,
                });
            }
            s
        }

        #[component]
        fn ProbePopoverStability() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.snapshot
                .set(snapshot_with_unread("Initial report", false));
            ui.utility_popup.set("notifications".into());
            view()
                .w(px(1200.))
                .h(px(900.))
                .on_press(move |_: &()| {
                    ui.snapshot
                        .set(snapshot_with_unread("Updated report", true));
                })
                .child(trigger_pill(ui))
                .child(popover(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(900.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbePopoverStability>());
        let initial = core.commit().expect("initial mount");
        let host = initial
            .items
            .iter()
            .find_map(|item| match item {
                MountItem::Insert { parent, child, .. } if *parent == ROOT_ID => Some(*child),
                _ => None,
            })
            .expect("root host node");
        let read_all_id: NodeId = initial
            .items
            .iter()
            .find_map(|item| match item {
                MountItem::Text { id, text, .. } if text.text == "✓ Read all" => Some(*id),
                _ => None,
            })
            .expect("expected `✓ Read all` in initial mount");

        assert!(core.component_event(host, EventKind::Press, &EventPayload::None));
        let update = core.commit().expect("update batch");
        assert!(
            !update
                .items
                .iter()
                .any(|item| matches!(item, MountItem::Delete { id } if *id == read_all_id)),
            "`✓ Read all` must not be deleted/recreated on background snapshot updates: {:?}",
            update.items
        );
        assert!(
            !update.items.iter().any(
                |item| matches!(item, MountItem::Text { text, .. } if text.text == "✓ Read all")
            ),
            "`✓ Read all` must not be re-emitted as a new Text node on background snapshot updates"
        );
    }
}
