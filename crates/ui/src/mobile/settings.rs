//! Mobile `Settings` tab: Hosts (🔒/⚠ security badges, reconnect, toggle, Diagnostics,
//! Remove, `+ Pair Machine`), Offline Sync queue & conflict resolution sheet, and Diagnostics.
use super::{
    MobileSheet, MobileState,
    chrome::{host_connection_status, mobile_switch, security_badge, trigger_haptic},
    host_security, mobile_hosts,
};
use crate::presenter::*;
use canban_types::{SyncAction, SyncItemState, SyncItemView};
use canban_ui_kit::{tokens::*, *};
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

pub(crate) fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let hosts_memo = computed(move || {
        mobile_hosts(ui)
            .into_iter()
            .map(|h| {
                let sec = host_security(ui, &h);
                (h, sec)
            })
            .collect::<Vec<_>>()
    });

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

    view()
        .size_full()
        .flex_col()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(
            view()
                .w_full()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .child(heading("Settings")),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .child(view().w_full().p_4().gap_4().child(dynamic(move || {
                    let hosts = hosts_memo.get();
                    let (total_sync, conflicts, failed) = sync_memo.get();

                    let mut hosts_col = view().w_full().gap_2().child(
                        view()
                            .w_full()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                label("HOSTS")
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(MUTED),
                            )
                            .child(compact_action("+ Pair Machine", Variant::Primary, move |_| {
                                state.open_pairing(None);
                            })),
                    );

                    for (host, sec) in hosts {
                        let (conn_label, conn_tone) = host_connection_status(&host);
                        let hid_toggle = host.id.clone();
                        let hid_reconn = host.id.clone();
                        let hid_diag = host.id.clone();
                        let hid_remove = host.id.clone();
                        let is_enabled = host.connection.is_connected()
                            || !matches!(host.connection, canban_types::HostConnectionState::Offline { error, .. } if error == "Disabled");

                        let mut card = panel()
                            .p_3()
                            .gap_2()
                            .child(
                                view()
                                    .w_full()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(label(host.name).font_weight(FontWeight::SEMIBOLD))
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_1p5()
                                            .child(security_badge(sec))
                                            .child(status(conn_label, conn_tone)),
                                    ),
                            )
                            .child(muted(host.endpoint_label).font_family(MONO_FONT))
                            .child(muted(sec.hint()))
                            .child(muted(format!(
                                "{} project(s) · {} active agent(s)",
                                host.project_count, host.active_agents
                            )));

                        if !host.is_local {
                            card = card.child(
                                view()
                                    .w_full()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .pt_1()
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_1p5()
                                            .child(compact_action(
                                                "Reconnect",
                                                Variant::Secondary,
                                                move |_| crate::hosts::reconnect_host(ui, Some(hid_reconn.clone())),
                                            ))
                                            .child(compact_action(
                                                "Diagnostics",
                                                Variant::Quiet,
                                                move |_| {
                                                    ui.settings_host.set(hid_diag.clone());
                                                    crate::diagnostics::refresh_diagnostics(ui);
                                                    state.open_sheet(MobileSheet::Diagnostics(
                                                        hid_diag.clone(),
                                                    ));
                                                },
                                            ))
                                            .child(compact_action(
                                                "Remove",
                                                Variant::Quiet,
                                                move |_| remove_host_action(ui, hid_remove.clone()),
                                            )),
                                    )
                                    .child(mobile_switch(
                                        move || is_enabled,
                                        "Enable host",
                                        move |next| toggle_host_enabled(ui, hid_toggle.clone(), next),
                                    )),
                            );
                        }
                        hosts_col = hosts_col.child(card);
                    }

                    let sync_tone = if conflicts > 0 || failed > 0 {
                        Tone::Attention
                    } else if total_sync > 0 {
                        Tone::Info
                    } else {
                        Tone::Success
                    };
                    let sync_summary = if conflicts > 0 {
                        format!("{total_sync} queued · {conflicts} conflict(s)")
                    } else if total_sync > 0 {
                        format!("{total_sync} pending mutation(s)")
                    } else {
                        "All hosts synchronized".to_owned()
                    };

                    let sync_section = view()
                        .w_full()
                        .gap_2()
                        .child(
                            label("OFFLINE SYNC")
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(MUTED),
                        )
                        .child(
                            panel()
                                .p_3()
                                .gap_2()
                                .child(
                                    view()
                                        .w_full()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(status(sync_summary, sync_tone))
                                        .child(compact_action(
                                            "Inspect Queue",
                                            Variant::Secondary,
                                            move |_| state.open_sheet(MobileSheet::SyncConflicts),
                                        )),
                                )
                                .child(muted(
                                    "Offline edits replay automatically when connectivity resumes; conflicts pause for your decision.",
                                )),
                        );

                    let prefs_section = view()
                        .w_full()
                        .gap_2()
                        .child(
                            label("MOBILE TERMINAL")
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(MUTED),
                        )
                        .child(
                            panel()
                                .p_3()
                                .gap_1()
                                .child(label("Touch gestures & accessory keys"))
                                .child(muted(
                                    "Pinch terminal surface to scale font (9–28pt); long-press lines to select & copy; tap Ctrl once for sticky modifier.",
                                )),
                        );

                    view()
                        .w_full()
                        .gap_4()
                        .child(hosts_col)
                        .child(sync_section)
                        .child(prefs_section)
                        .into_element()
                }))),
        )
}

pub(crate) fn sync_conflicts_sheet_body(ui: Ui) -> impl IntoElement {
    let items_memo = computed(move || sync_items_for_ui(ui));

    dynamic(move || {
        let items = items_memo.get();
        if items.is_empty() {
            return panel()
                .p_4()
                .gap_1()
                .child(label("Sync queue empty"))
                .child(muted(
                    "No pending edits or sync conflicts across paired hosts.",
                ))
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
            let mut card = panel()
                .p_3()
                .gap_2()
                .child(
                    view()
                        .w_full()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            label(format!("{} · {}", item.host_name, item.card_title))
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .flex_1()
                                .min_w(px(0.)),
                        )
                        .child(status(state_lbl, tone)),
                )
                .child(muted(format!("{} — {}", item.kind, item.message)));

            for field_name in &item.conflict_fields {
                let mine = item.mine.get(field_name).map(String::as_str).unwrap_or("");
                let server = item
                    .server
                    .get(field_name)
                    .map(String::as_str)
                    .unwrap_or("");
                card = card.child(
                    view()
                        .p_2()
                        .rounded(px(6.))
                        .bg(INSET)
                        .gap_1()
                        .child(label(field_name.clone()).text_size(px(12.)))
                        .child(muted(format!("Mine: {mine}")))
                        .child(muted(format!("Server: {server}"))),
                );
            }

            let hid_a = item.host_id.clone();
            let mid_a = item.mutation_id.clone();
            let hid_b = item.host_id.clone();
            let mid_b = item.mutation_id.clone();

            let actions = match item.state {
                SyncItemState::Conflict => view()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .child(compact_action(
                        "Overwrite remote",
                        Variant::Primary,
                        move |_| {
                            dispatch_sync(ui, hid_a.clone(), mid_a.clone(), SyncAction::UseMine);
                        },
                    ))
                    .child(compact_action(
                        "Discard mine",
                        Variant::Secondary,
                        move |_| {
                            dispatch_sync(ui, hid_b.clone(), mid_b.clone(), SyncAction::Discard);
                        },
                    )),
                SyncItemState::Failed => view()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .child(compact_action("Retry", Variant::Primary, move |_| {
                        dispatch_sync(ui, hid_a.clone(), mid_a.clone(), SyncAction::Retry);
                    }))
                    .child(compact_action("Discard", Variant::Quiet, move |_| {
                        dispatch_sync(ui, hid_b.clone(), mid_b.clone(), SyncAction::Discard);
                    })),
                SyncItemState::Queued => view().flex_row().gap_2().child(compact_action(
                    "Discard",
                    Variant::Quiet,
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

    view().w_full().gap_3().child(dynamic(move || {
        let (diag, logs, loading) = model.get();
        let hid_refresh = host_id.clone();
        if loading && diag.is_null() {
            return loading_panel("Loading host diagnostics…", "Querying remote server")
                .into_element();
        }
        let server = &diag["server"];
        let storage = &diag["storage"];
        let log_text = logs["text"].as_str().unwrap_or("No recent logs captured.");
        view()
            .w_full()
            .gap_3()
            .child(
                view()
                    .w_full()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(label(
                        server["hostname"]
                            .as_str()
                            .unwrap_or(host_id.as_str())
                            .to_owned(),
                    ))
                    .child(compact_action("Refresh", Variant::Secondary, move |_| {
                        ui.settings_host.set(hid_refresh.clone());
                        crate::diagnostics::refresh_diagnostics(ui);
                    })),
            )
            .child(
                panel()
                    .p_3()
                    .gap_1()
                    .child(muted(format!(
                        "API v{} · Version {}",
                        server["api_version"].as_u64().unwrap_or(0),
                        server["build"]["version"].as_str().unwrap_or("unknown"),
                    )))
                    .child(muted(format!(
                        "Platform: {}",
                        server["build"]["platform"].as_str().unwrap_or("unknown"),
                    )))
                    .child(muted(format!(
                        "Storage: {} ({} project(s), {} active card(s))",
                        if storage["healthy"] == true {
                            "Healthy"
                        } else {
                            "Unknown"
                        },
                        storage["project_count"].as_u64().unwrap_or(0),
                        storage["active_card_count"].as_u64().unwrap_or(0),
                    ))),
            )
            .child(
                panel()
                    .p_3()
                    .gap_1()
                    .child(label("Recent daemon logs"))
                    .child(muted(log_text.to_owned()).font_family(MONO_FONT)),
            )
            .into_element()
    }))
}
