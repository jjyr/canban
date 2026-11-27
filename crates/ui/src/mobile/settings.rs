//! Mobile `Settings` tab (Inset Grouped Hosts / Sync / Terminal Gestures),
//! pushed `HostDetail` management page (native Switch, Reconnect, Diagnostics, confirmed Remove),
//! Offline Sync Queue sheet, and Host Diagnostics sheet.
use super::{
    MobileSheet, MobileState,
    chrome::{
        MOBILE_BODY, MOBILE_CAPTION, MOBILE_FOOTNOTE, MOBILE_RADIUS, MOBILE_SUBHEAD,
        confirm_destructive, disclosure_chevron, grouped_card, grouped_card_with_inset,
        host_connection_status, icon_badge, is_mobile_os, mobile_footnote, mobile_inline_button,
        mobile_subhead, mobile_switch, mobile_title, section_header, tab_scroll_bottom_pad,
        trigger_haptic,
    },
    is_tailscale_auto_host, mobile_hosts,
};
use crate::presenter::*;
use canban_types::{HostConnectionState, HostSummary, SyncAction, SyncItemState, SyncItemView};
use canban_ui_kit::{tokens::*, *};
use nsh_base::nav_page;
use nsh_core::{Haptic, prelude::*};

fn sync_items_for_ui(ui: Ui) -> Vec<SyncItemView> {
    if !crate::backend::is_connected() {
        return ui.sync_preview.get();
    }
    let _ = ui.snapshot.get();
    let _ = ui.hosts.get();
    crate::backend::sync_items()
}

fn dispatch_sync(ui: Ui, host_id: String, mutation_id: String, action: SyncAction) {
    trigger_haptic(Haptic::Selection);
    if !crate::backend::is_connected() {
        ui.sync_preview.update(|items| {
            items.retain(|i| i.mutation_id != mutation_id);
        });
        ui.notify("Gallery preview · sync action applied");
        return;
    }
    ui.spawn(async move {
        let res =
            blocking(move || crate::backend::sync_action(&host_id, &mutation_id, action)).await;
        match res {
            Ok(Ok(())) => {
                ui.notify("Sync queue updated");
                refresh(ui);
            }
            Ok(Err(e)) => {
                ui.operation_error.try_set(format!("{e:#}"));
            }
            Err(e) => {
                ui.operation_error.try_set(e.to_string());
            }
        }
    });
}

fn toggle_host_enabled(ui: Ui, host_id: String, next_enabled: bool) {
    if let Some(backend) = crate::backend::get().filter(|b| b.has_host_registry()) {
        ui.spawn(async move {
            let res = blocking(move || {
                backend.set_host_enabled(&host_id, next_enabled)?;
                let sums = backend.host_summaries();
                let snap = backend.merged_snapshot()?;
                anyhow::Ok((sums, snap))
            })
            .await;
            if let Ok(Ok((sums, snap))) = res {
                let _ = ui.hosts.try_set(sums);
                ui.accept_snapshot(snap);
            }
        });
    } else {
        ui.notify(if next_enabled {
            "Gallery preview · Host enabled"
        } else {
            "Gallery preview · Host paused"
        });
    }
}

fn remove_host_action(ui: Ui, host_id: String) {
    if let Some(backend) = crate::backend::get().filter(|b| b.has_host_registry()) {
        ui.spawn(async move {
            let res = blocking(move || {
                backend.remove_host(&host_id)?;
                let sums = backend.host_summaries();
                let snap = backend.merged_snapshot()?;
                anyhow::Ok((sums, snap))
            })
            .await;
            if let Ok(Ok((sums, snap))) = res {
                let _ = ui.hosts.try_set(sums);
                ui.accept_snapshot(snap);
                ui.notify("Host removed");
            }
        });
    } else {
        ui.hosts.update(|hs| hs.retain(|h| h.id != host_id));
        ui.notify("Gallery preview · Host removed");
    }
}

fn host_error_detail(host: &HostSummary) -> Option<(String, bool)> {
    match &host.connection {
        HostConnectionState::Offline { error, .. } if !error.is_empty() && error != "Disabled" => {
            Some((error.clone(), false))
        }
        HostConnectionState::AuthFailed { error }
        | HostConnectionState::KeyMismatch { error, .. } => Some((error.clone(), true)),
        HostConnectionState::ApiMismatch {
            app_api,
            server_api,
            ..
        } => Some((
            format!("API mismatch (app v{app_api}, host v{server_api})"),
            false,
        )),
        _ => None,
    }
}

fn key_value_row(title: &'static str, value: impl Into<SharedString>) -> AnyElement {
    view()
        .w_full()
        .min_h(px(50.))
        .px_4()
        .py_2p5()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            text(title)
                .text_size(px(15.5))
                .text_color(TEXT)
                .flex_shrink_0(),
        )
        .child(
            text(value.into())
                .text_size(px(14.5))
                .text_color(MUTED)
                .truncate()
                .min_w(px(0.)),
        )
        .into_element()
}

pub(crate) fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let hosts_memo = computed(move || mobile_hosts(ui));

    let sync_memo = computed(move || {
        let items = sync_items_for_ui(ui);
        let conflicts = items
            .iter()
            .filter(|i| i.state == SyncItemState::Conflict)
            .count();
        let failed = items
            .iter()
            .filter(|i| i.state == SyncItemState::Failed)
            .count();
        (items.len(), conflicts, failed)
    });

    scroll().size_full().bg(BACKGROUND).child(
        view()
            .w_full()
            .px_4()
            .pt_2()
            .pb(px(tab_scroll_bottom_pad()))
            .gap_6()
            .child(dynamic(move || {
                let hosts = hosts_memo.get();
                let (total_sync, conflicts, failed) = sync_memo.get();

                let mut host_rows: Vec<AnyElement> = hosts
                    .into_iter()
                    .map(|host| {
                        let (conn_label, conn_tone) = host_connection_status(&host);
                        let (badge_bg, badge_fg) = tone_colors(conn_tone);
                        let hid = host.id.clone();
                        let subtitle = if let Some((err, _)) = host_error_detail(&host) {
                            err
                        } else {
                            format!(
                                "{conn_label} · {} project{}",
                                host.project_count,
                                if host.project_count == 1 { "" } else { "s" }
                            )
                        };
                        let err_is_danger =
                            host_error_detail(&host).is_some_and(|(_, is_danger)| is_danger);
                        let has_err = host_error_detail(&host).is_some();

                        press_target()
                            .w_full()
                            .min_h(px(64.))
                            .px_4()
                            .py_2p5()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .cursor_pointer()
                            .active(|s| s.opacity(0.75))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_3()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .child(icon_badge(Icon::Server, badge_fg, badge_bg))
                                    .child(
                                        view()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .gap_0p5()
                                            .child(
                                                text(host.name)
                                                    .text_size(px(16.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(TEXT)
                                                    .truncate(),
                                            )
                                            .child(
                                                text(subtitle)
                                                    .text_size(px(13.5))
                                                    .text_color(if err_is_danger {
                                                        DANGER
                                                    } else if has_err {
                                                        ATTENTION
                                                    } else {
                                                        MUTED
                                                    })
                                                    .truncate(),
                                            ),
                                    ),
                            )
                            .child(disclosure_chevron())
                            .on_press(move |_: &()| {
                                trigger_haptic(Haptic::Selection);
                                state.push_host_detail(hid.clone());
                            })
                            .into_element()
                    })
                    .collect();

                let has_any_host = !host_rows.is_empty();
                host_rows.push(
                    press_target()
                        .w_full()
                        .min_h(px(54.))
                        .px_4()
                        .py_2p5()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .cursor_pointer()
                        .active(|s| s.opacity(0.75))
                        .child(icon_badge(Icon::Plus, ACCENT, SELECTED))
                        .child(
                            text("Pair Machine")
                                .text_size(px(16.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ACCENT),
                        )
                        .on_press(move |_: &()| {
                            trigger_haptic(Haptic::Selection);
                            state.open_pairing(None);
                        })
                        .into_element(),
                );
                if has_any_host {
                    let disc_count = state.discovered_hosts.with(|h| h.len());
                    let disc_badge = if disc_count > 0 {
                        Some(status(format!("{disc_count} found"), Tone::Info))
                    } else {
                        None
                    };
                    host_rows.push(
                        press_target()
                            .w_full()
                            .min_h(px(54.))
                            .px_4()
                            .py_2p5()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .cursor_pointer()
                            .active(|s| s.opacity(0.75))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_3()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .child(icon_badge(Icon::Refresh, ACCENT, SELECTED))
                                    .child(
                                        text("Discover on Tailnet")
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(TEXT)
                                            .truncate(),
                                    ),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .flex_shrink_0()
                                    .children(disc_badge)
                                    .child(disclosure_chevron()),
                            )
                            .on_press(move |_: &()| {
                                trigger_haptic(Haptic::Selection);
                                state.open_sheet(MobileSheet::DiscoverHosts);
                                super::pairing::run_tailnet_discovery(ui, state, None, false);
                            })
                            .into_element(),
                    );
                }

                let hosts_section = view()
                    .w_full()
                    .child(section_header("HOSTS", None))
                    .child(grouped_card_with_inset(host_rows, 60.0));

                let sync_tone = if conflicts > 0 || failed > 0 {
                    Tone::Attention
                } else if total_sync > 0 {
                    Tone::Info
                } else {
                    Tone::Success
                };
                let (sync_bg, sync_fg) = tone_colors(sync_tone);
                let sync_badge_text = if conflicts > 0 {
                    format!("{total_sync} · {conflicts} issue")
                } else if total_sync > 0 {
                    format!("{total_sync} queued")
                } else {
                    "Synced".to_owned()
                };

                let sync_row = press_target()
                    .w_full()
                    .min_h(px(56.))
                    .px_4()
                    .py_2p5()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .cursor_pointer()
                    .active(|s| s.opacity(0.75))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .flex_1()
                            .min_w(px(0.))
                            .child(icon_badge(Icon::Refresh, sync_fg, sync_bg))
                            .child(
                                text("Offline Sync Queue")
                                    .text_size(px(16.))
                                    .text_color(TEXT)
                                    .truncate(),
                            ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .flex_shrink_0()
                            .child(status(sync_badge_text, sync_tone))
                            .child(disclosure_chevron()),
                    )
                    .on_press(move |_: &()| {
                        trigger_haptic(Haptic::Selection);
                        state.open_sheet(MobileSheet::SyncConflicts);
                    })
                    .into_element();

                let sync_section = view()
                    .w_full()
                    .child(section_header("SYNC", None))
                    .child(grouped_card(vec![sync_row]))
                    .child(
                        view().px_2().pt_2().child(mobile_footnote(
                            "Offline edits replay automatically when connectivity resumes; conflicts pause for your decision.",
                        )),
                    );

                let gestures_section = view()
                    .w_full()
                    .child(section_header("TERMINAL GESTURES", None))
                    .child(grouped_card(vec![
                        key_value_row("Pinch to zoom", "9–28 pt"),
                        key_value_row("Sticky Ctrl", "Tap once"),
                        key_value_row("Select & copy", "Long-press line"),
                    ]));

                view()
                    .w_full()
                    .gap_6()
                    .child(hosts_section)
                    .child(sync_section)
                    .child(gestures_section)
                    .into_element()
            })),
    )
}

/// Pushed secondary route displaying connection status, switch toggle, reconnect, diagnostics, and removal.
pub(crate) fn host_detail_page(ui: Ui, state: MobileState, host_id: String) -> impl IntoElement {
    let hid_lookup = host_id.clone();
    let host_memo = computed(move || mobile_hosts(ui).into_iter().find(|h| h.id == hid_lookup));

    let title_fn = move || {
        host_memo
            .get()
            .map(|h| h.name)
            .unwrap_or_else(|| "Host".to_owned())
    };

    let desktop_header = view()
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
                    text("Settings")
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ACCENT),
                )
                .on_press(move |_: &()| state.pop()),
        )
        .child(mobile_title(title_fn).truncate().flex_1().min_w(px(0.)))
        .child(view().w(px(56.)).flex_shrink_0());

    let mut page = nav_page()
        .key(format!("mobile-host:{host_id}"))
        .title(title_fn)
        .header_large_title(false)
        .header_shadow_visible(false)
        .header_style_with(|s| s.text_color(TEXT).tint_color(ACCENT))
        .hides_bottom_bar_when_pushed(true)
        .on_dismiss(move |_: &()| state.pop());

    if !is_mobile_os() {
        page = page.header(desktop_header);
    }

    page.child(
        scroll()
            .size_full()
            .bg(BACKGROUND)
            .child(view().w_full().p_4().pb(px(40.)).gap_6().child(dynamic(move || {
                let Some(host) = host_memo.get() else {
                    return mobile_subhead("Host removed or unavailable.").into_element();
                };
                let (conn_label, conn_tone) = host_connection_status(&host);
                let is_enabled = host.connection.is_connected()
                    || !matches!(
                        &host.connection,
                        HostConnectionState::Offline { error, .. } if error == "Disabled"
                    );
                let err_detail = host_error_detail(&host);

                let status_row = view()
                    .w_full()
                    .min_h(px(52.))
                    .px_4()
                    .py_2p5()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(text("Status").text_size(px(MOBILE_BODY)).text_color(TEXT))
                    .child(status(conn_label, conn_tone))
                    .into_element();

                let address_row = view()
                    .w_full()
                    .min_h(px(52.))
                    .px_4()
                    .py_2p5()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(text("Address").text_size(px(MOBILE_BODY)).text_color(TEXT))
                    .child(
                        text(host.endpoint_label.clone())
                            .text_size(px(MOBILE_SUBHEAD))
                            .font_family(MONO_FONT)
                            .text_color(MUTED)
                            .truncate()
                            .min_w(px(0.)),
                    )
                    .into_element();

                let transport_hint = if is_tailscale_auto_host(&host.id) {
                    "End-to-end encrypted (Noise) · automatic Tailscale same-account sign-in"
                } else {
                    "End-to-end encrypted (Noise) with pinned host key"
                };
                let transport_row = view()
                    .w_full()
                    .min_h(px(58.))
                    .px_4()
                    .py_2p5()
                    .gap_1()
                    .child(
                        view()
                            .w_full()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(text("Transport").text_size(px(MOBILE_BODY)).text_color(TEXT))
                            .child(
                                text(host.transport_label.clone())
                                    .text_size(px(MOBILE_SUBHEAD))
                                    .text_color(MUTED),
                            ),
                    )
                    .child(mobile_footnote(transport_hint))
                    .into_element();

                let workload_row = key_value_row(
                    "Workload",
                    format!(
                        "{} project(s) · {} active agent(s)",
                        host.project_count, host.active_agents
                    ),
                );

                let mut conn_rows = vec![status_row, address_row, transport_row];
                if let Some(fp) = host.server_pubkey_fingerprint.clone() {
                    conn_rows.push(
                        view()
                            .w_full()
                            .min_h(px(50.))
                            .px_4()
                            .py_2p5()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                text("Host Key")
                                    .text_size(px(15.5))
                                    .text_color(TEXT)
                                    .flex_shrink_0(),
                            )
                            .child(
                                text(fp)
                                    .text_size(px(13.5))
                                    .font_family(MONO_FONT)
                                    .text_color(MUTED)
                                    .truncate()
                                    .min_w(px(0.)),
                            )
                            .into_element(),
                    );
                }
                conn_rows.push(workload_row);

                let mut col = view().w_full().gap_6().child(
                    view()
                        .w_full()
                        .child(section_header("CONNECTION", None))
                        .child(grouped_card(conn_rows)),
                );

                if let Some((err_text, is_danger)) = err_detail {
                    col = col.child(notice(
                        err_text,
                        if is_danger { Tone::Danger } else { Tone::Attention },
                    ));
                }

                if !host.is_local {
                    let hid_toggle = host.id.clone();
                    let hid_reconn = host.id.clone();
                    let hid_diag = host.id.clone();
                    let hid_remove = host.id.clone();
                    let host_name_remove = host.name.clone();

                    let enabled_row = view()
                        .w_full()
                        .min_h(px(54.))
                        .px_4()
                        .py_2()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(text("Enabled").text_size(px(MOBILE_BODY)).text_color(TEXT))
                        .child(mobile_switch(
                            move || is_enabled,
                            "Enable host",
                            move |next| toggle_host_enabled(ui, hid_toggle.clone(), next),
                        ))
                        .into_element();

                    let reconnect_row = press_target()
                        .w_full()
                        .min_h(px(52.))
                        .px_4()
                        .py_2p5()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .cursor_pointer()
                        .active(|s| s.opacity(0.75))
                        .child(icon_badge(Icon::Refresh, ACCENT, SELECTED))
                        .child(
                            text("Reconnect Now")
                                .text_size(px(MOBILE_BODY))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ACCENT),
                        )
                        .on_press(move |_: &()| {
                            trigger_haptic(Haptic::Selection);
                            crate::hosts::reconnect_host(ui, Some(hid_reconn.clone()));
                        })
                        .into_element();

                    let diag_row = press_target()
                        .w_full()
                        .min_h(px(52.))
                        .px_4()
                        .py_2p5()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .cursor_pointer()
                        .active(|s| s.opacity(0.75))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .child(icon_badge(Icon::Diagnostics, TEXT, INSET))
                                .child(
                                    text("Diagnostics & Logs")
                                        .text_size(px(MOBILE_BODY))
                                        .text_color(TEXT),
                                ),
                        )
                        .child(disclosure_chevron())
                        .on_press(move |_: &()| {
                            trigger_haptic(Haptic::Selection);
                            ui.settings_host.set(hid_diag.clone());
                            crate::diagnostics::refresh_diagnostics(ui);
                            state.open_sheet(MobileSheet::Diagnostics(hid_diag.clone()));
                        })
                        .into_element();

                    col = col.child(
                        view()
                            .w_full()
                            .child(section_header("ACTIONS", None))
                            .child(grouped_card_with_inset(
                                vec![enabled_row, reconnect_row, diag_row],
                                16.0,
                            )),
                    );

                    let remove_row = press_target()
                        .w_full()
                        .min_h(px(52.))
                        .px_4()
                        .py_2p5()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .active(|s| s.opacity(0.75))
                        .child(
                            text("Remove Host")
                                .text_size(px(MOBILE_BODY))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(DANGER),
                        )
                        .on_press(move |_: &()| {
                            trigger_haptic(Haptic::Selection);
                            let hid_confirm = hid_remove.clone();
                            confirm_destructive(
                                ui,
                                format!("Remove {host_name_remove}?"),
                                "This disconnects this machine and clears its offline queue from this device.",
                                "Remove",
                                move || {
                                    remove_host_action(ui, hid_confirm.clone());
                                    state.pop();
                                },
                            );
                        })
                        .into_element();

                    col = col.child(grouped_card(vec![remove_row]));
                }

                col.into_element()
            }))),
    )
}

pub(crate) fn sync_conflicts_sheet_body(ui: Ui) -> impl IntoElement {
    let items_memo = computed(move || sync_items_for_ui(ui));

    dynamic(move || {
        let items = items_memo.get();
        if items.is_empty() {
            return super::chrome::empty_state(
                Icon::Check,
                Tone::Success,
                "Sync queue empty",
                "No pending edits or sync conflicts across paired hosts.",
                None,
            )
            .into_element();
        }
        let mut col = view().w_full().gap_3();
        for item in items {
            let (state_lbl, tone) = match item.state {
                SyncItemState::Queued => ("Queued", Tone::Neutral),
                SyncItemState::InFlight => ("Syncing…", Tone::Info),
                SyncItemState::Failed => ("Failed", Tone::Danger),
                SyncItemState::Conflict => ("Conflict", Tone::Attention),
            };
            let mut card = view()
                .w_full()
                .p_4()
                .gap_3()
                .rounded(px(MOBILE_RADIUS))
                .border_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .child(
                    view()
                        .w_full()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            mobile_title(format!("{} · {}", item.host_name, item.card_title))
                                .truncate()
                                .flex_1()
                                .min_w(px(0.)),
                        )
                        .child(status(state_lbl, tone)),
                )
                .child(mobile_subhead(format!("{} — {}", item.kind, item.message)));

            for field_name in &item.conflict_fields {
                let mine = item.mine.get(field_name).map(String::as_str).unwrap_or("");
                let server = item
                    .server
                    .get(field_name)
                    .map(String::as_str)
                    .unwrap_or("");
                card = card.child(
                    view()
                        .w_full()
                        .p_3()
                        .rounded(px(10.))
                        .bg(INSET)
                        .gap_1()
                        .child(
                            text(field_name.clone())
                                .text_size(px(MOBILE_FOOTNOTE))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(TEXT),
                        )
                        .child(mobile_footnote(format!("Mine: {mine}")))
                        .child(mobile_footnote(format!("Server: {server}"))),
                );
            }

            let hid_a = item.host_id.clone();
            let mid_a = item.mutation_id.clone();
            let hid_b = item.host_id.clone();
            let mid_b = item.mutation_id.clone();

            let actions = match item.state {
                SyncItemState::Conflict => view()
                    .w_full()
                    .flex_row()
                    .gap_2()
                    .child(
                        mobile_inline_button("Discard Mine", Variant::Secondary, move |_| {
                            dispatch_sync(ui, hid_b.clone(), mid_b.clone(), SyncAction::Discard);
                        })
                        .flex_1(),
                    )
                    .child(
                        mobile_inline_button("Overwrite Remote", Variant::Primary, move |_| {
                            dispatch_sync(ui, hid_a.clone(), mid_a.clone(), SyncAction::UseMine);
                        })
                        .flex_1(),
                    ),
                SyncItemState::Failed => view()
                    .w_full()
                    .flex_row()
                    .gap_2()
                    .child(
                        mobile_inline_button("Discard", Variant::Secondary, move |_| {
                            dispatch_sync(ui, hid_b.clone(), mid_b.clone(), SyncAction::Discard);
                        })
                        .flex_1(),
                    )
                    .child(
                        mobile_inline_button("Retry", Variant::Primary, move |_| {
                            dispatch_sync(ui, hid_a.clone(), mid_a.clone(), SyncAction::Retry);
                        })
                        .flex_1(),
                    ),
                SyncItemState::Queued => view().w_full().child(mobile_inline_button(
                    "Discard",
                    Variant::Secondary,
                    move |_| {
                        dispatch_sync(ui, hid_b.clone(), mid_b.clone(), SyncAction::Discard);
                    },
                )),
                SyncItemState::InFlight => view(),
            };
            col = col.child(card.child(actions));
        }
        col.into_element()
    })
}

pub(crate) fn diagnostics_sheet_body(ui: Ui, host_id: String) -> impl IntoElement {
    let model = computed(move || {
        let diag = ui.diagnostics.get();
        let logs = ui.diagnostics_logs.get();
        let loading = ui.diagnostics_loading.get();
        (diag, logs, loading)
    });

    view().w_full().gap_4().child(dynamic(move || {
        let (diag, logs, loading) = model.get();
        let hid_refresh = host_id.clone();
        if loading && diag.is_null() {
            return loading_panel("Loading host diagnostics…", "Querying remote server")
                .into_element();
        }
        let server = &diag["server"];
        let storage = &diag["storage"];
        let log_text = logs["text"].as_str().unwrap_or("No recent logs captured.");

        let refresh_btn = press_target()
            .min_h(px(32.))
            .px_2()
            .items_center()
            .justify_center()
            .child(
                text("Refresh")
                    .text_size(px(13.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ACCENT),
            )
            .on_press(move |_: &()| {
                trigger_haptic(Haptic::Selection);
                ui.settings_host.set(hid_refresh.clone());
                crate::diagnostics::refresh_diagnostics(ui);
            })
            .into_element();

        let hostname = server["hostname"]
            .as_str()
            .unwrap_or(host_id.as_str())
            .to_owned();
        let ver = format!(
            "API v{} · {}",
            server["api_version"].as_u64().unwrap_or(0),
            server["build"]["version"].as_str().unwrap_or("unknown"),
        );
        let plat = server["build"]["platform"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned();
        let storage_str = format!(
            "{} ({} project(s), {} active)",
            if storage["healthy"] == true {
                "Healthy"
            } else {
                "Unknown"
            },
            storage["project_count"].as_u64().unwrap_or(0),
            storage["active_card_count"].as_u64().unwrap_or(0),
        );

        view()
            .w_full()
            .gap_5()
            .child(
                view()
                    .w_full()
                    .child(section_header("DAEMON", Some(refresh_btn)))
                    .child(grouped_card(vec![
                        key_value_row("Host", hostname),
                        key_value_row("Build", ver),
                        key_value_row("Platform", plat),
                        key_value_row("Storage", storage_str),
                    ])),
            )
            .child(
                view()
                    .w_full()
                    .child(section_header("RECENT LOGS", None))
                    .child(
                        view()
                            .w_full()
                            .p_3p5()
                            .rounded(px(MOBILE_RADIUS))
                            .border_1()
                            .border_color(BORDER)
                            .bg(SURFACE)
                            .child(
                                text(log_text.to_owned())
                                    .text_size(px(MOBILE_CAPTION))
                                    .line_height(px(17.))
                                    .font_family(MONO_FONT)
                                    .text_color(MUTED),
                            ),
                    ),
            )
            .into_element()
    }))
}
