//! Dedicated `Settings -> Diagnostics` view inspecting connection transport, daemon & SQLite health,
//! Herdr Core supervision, toolchain / Agent binaries, and live `server.log` / `updater.log` tails.
use crate::presenter::*;
use canban_types::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::{Value, json};

pub(crate) fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn format_uptime(ms: u64) -> String {
    let secs = ms / 1000;
    let mins = secs / 60;
    let hours = mins / 60;
    let days = hours / 24;
    if days > 0 {
        format!("{}d {}h {}m", days, hours % 24, mins % 60)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins % 60)
    } else if mins > 0 {
        format!("{}m {}s", mins, secs % 60)
    } else {
        format!("{secs}s")
    }
}

pub fn fixture_diagnostics(host_id: &str) -> (Value, Value) {
    let is_local = host_id.is_empty() || host_id == "local";
    let status = if is_local {
        json!({
            "checked_at": 1_763_200_000_000_u64,
            "server": {
                "server_id": "srv-macbook-local",
                "hostname": "macbook-pro",
                "pid": 48210,
                "uptime_ms": 14_520_000,
                "home": "/Users/demo/.canban",
                "socket": "/Users/demo/.canban/app.sock",
                "listen_config": { "tailscale": false, "port": 7680 },
                "bound_tcp": [],
                "transport_encryption": "Noise_XX_25519_ChaChaPoly_BLAKE2s",
                "server_pubkey_fingerprint": "A1B2-C3D4-E5F6",
                "api_version": crate::backend::desktop_api(),
                "build": {
                    "version": "20261115-1",
                    "commit": "abcdef0",
                    "schema": 1,
                    "api": crate::backend::desktop_api(),
                    "platform": "macos-aarch64",
                }
            },
            "storage": {
                "healthy": true,
                "journal_mode": "wal",
                "db_path": "/Users/demo/.canban/canban.sqlite",
                "db_size_bytes": 4_194_304,
                "wal_size_bytes": 131_072,
                "project_count": 2,
                "active_card_count": 5,
                "active_terminal_count": 2,
            },
            "herdr": {
                "check": { "installed": true, "ready": true },
                "core_id": "core-local-mac",
                "observer_connected": true,
                "observed_terminals": 2,
                "session": "canban",
            },
            "toolchain": {
                "git": { "available": true, "path": "/usr/bin/git", "version": "git version 2.47.0" },
                "python3": { "available": true, "path": "/usr/bin/python3", "version": "Python 3.12.6" },
                "tailscale": { "available": true, "ipv4": "100.84.0.1" },
                "service_manager": { "kind": "launchd", "installed": true, "active": true },
                "profiles": [
                    { "id": "developer", "identity": true, "command": "codex", "available": true, "resolved_path": "/Users/demo/.local/bin/codex" },
                    { "id": "reviewer", "identity": true, "command": "claude", "available": true, "resolved_path": "/Users/demo/.local/bin/claude" }
                ]
            }
        })
    } else {
        json!({
            "checked_at": 1_763_200_000_000_u64,
            "server": {
                "server_id": "srv-home-linux",
                "hostname": "home-linux",
                "pid": 1942,
                "uptime_ms": 604_800_000,
                "home": "/home/builder/.canban",
                "socket": "/home/builder/.canban/app.sock",
                "listen_config": { "tailscale": true, "port": 7680 },
                "bound_tcp": ["100.84.0.2:7680"],
                "transport_encryption": "Noise_XX_25519_ChaChaPoly_BLAKE2s",
                "server_pubkey_fingerprint": "9F8E-7D6C-5B4A",
                "api_version": crate::backend::desktop_api(),
                "build": {
                    "version": "20261115-1",
                    "commit": "abcdef0",
                    "schema": 1,
                    "api": crate::backend::desktop_api(),
                    "platform": "linux-x86_64",
                }
            },
            "storage": {
                "healthy": true,
                "journal_mode": "wal",
                "db_path": "/home/builder/.canban/canban.sqlite",
                "db_size_bytes": 12_582_912,
                "wal_size_bytes": 262_144,
                "project_count": 3,
                "active_card_count": 8,
                "active_terminal_count": 4,
            },
            "herdr": {
                "check": { "installed": true, "ready": true },
                "core_id": "core-home-linux",
                "observer_connected": true,
                "observed_terminals": 4,
                "session": "canban",
            },
            "toolchain": {
                "git": { "available": true, "path": "/usr/bin/git", "version": "git version 2.45.2" },
                "python3": { "available": true, "path": "/usr/bin/python3", "version": "Python 3.12.3" },
                "tailscale": { "available": true, "ipv4": "100.84.0.2" },
                "service_manager": { "kind": "systemd-user", "installed": true, "active": true, "linger": true },
                "profiles": [
                    { "id": "developer", "identity": true, "command": "codex", "available": true, "resolved_path": "/home/builder/.local/bin/codex" },
                    { "id": "reviewer", "identity": true, "command": "claude", "available": true, "resolved_path": "/home/builder/.local/bin/claude" }
                ]
            }
        })
    };

    let logs = json!({
        "exists": true,
        "path": if is_local { "/Users/demo/.canban/server.log" } else { "/home/builder/.canban/server.log" },
        "size_bytes": 2048,
        "text": "[2026-11-15T09:12:04Z INFO] AppServer ready (API v52, WAL journal)\n[2026-11-15T09:12:05Z INFO] Herdr Core observer subscribed (session=canban)\n[2026-11-15T09:45:19Z INFO] Workflow step `develop` admitted on Card #40 (@developer)"
    });
    (status, logs)
}

pub fn refresh_diagnostics(ui: Ui) {
    let host_id = ui.settings_host.get_untracked();
    let log_src = {
        let cur = ui.diagnostics_log_source.get_untracked();
        if cur.is_empty() {
            "server".to_owned()
        } else {
            cur
        }
    };
    if !crate::backend::is_connected() {
        let (st, lg) = fixture_diagnostics(&host_id);
        ui.diagnostics.set(st);
        ui.diagnostics_logs.set(lg);
        return;
    }
    if ui.diagnostics_loading.get_untracked() {
        return;
    }
    ui.diagnostics_loading.set(true);
    ui.spawn(async move {
        let h1 = host_id.clone();
        let h2 = host_id.clone();
        let src = log_src.clone();
        let res = blocking(move || {
            let st = crate::routed_call("diagnostics.status", json!({ "host_id": h1 }), None);
            let lg = crate::routed_call(
                "diagnostics.logs",
                json!({ "host_id": h2, "source": src, "lines": 180 }),
                None,
            );
            (st, lg)
        })
        .await;
        let _ = ui.diagnostics_loading.try_set(false);
        if let Ok((st_res, lg_res)) = res {
            match st_res {
                Ok(v) => {
                    let _ = ui.diagnostics.try_set(v);
                }
                Err(e) => {
                    let _ = ui.diagnostics.try_set(json!({
                        "error": format!("{e:#}")
                    }));
                }
            }
            match lg_res {
                Ok(v) => {
                    let _ = ui.diagnostics_logs.try_set(v);
                }
                Err(e) => {
                    let _ = ui.diagnostics_logs.try_set(json!({
                        "exists": false,
                        "text": format!("Unable to read logs: {e:#}")
                    }));
                }
            }
        }
    });
}

fn mono_val(val: impl Into<String>) -> Text {
    label(val.into())
        .font_family(MONO_FONT)
        .text_size(px(11.5))
        .truncate()
}

pub fn section_view(ui: Ui) -> impl IntoElement {
    let last_inspected = stored(ui.settings_host.get_untracked());
    effect(move || {
        let cur = ui.settings_host.get();
        if last_inspected.get() != cur {
            last_inspected.replace(cur);
            ui.diagnostics.set(Value::Null);
            ui.diagnostics_logs.set(Value::Null);
            refresh_diagnostics(ui);
        } else if ui.diagnostics.with_untracked(Value::is_null) {
            refresh_diagnostics(ui);
        }
    });

    settings_section(
        "Diagnostics & Runtime",
        "Transport connectivity, AppServer & SQLite health, Herdr Core supervision, toolchain binaries, and daemon logs.",
    )
    .trailing(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1p5()
            .child(compact_action(
                move || {
                    if ui.diagnostics_loading.get() {
                        "Running…".to_owned()
                    } else {
                        "Run Checks".to_owned()
                    }
                },
                Variant::Primary,
                move |_| refresh_diagnostics(ui),
            ))
            .child(compact_action(
                "Open Host Terminal",
                Variant::Secondary,
                move |_| {
                    crate::hosts::open_host_terminal(ui, ui.settings_host.get_untracked());
                },
            ))
            .child(compact_action(
                "Copy Report",
                Variant::Quiet,
                move |_| {
                    let report = json!({
                        "host_id": ui.settings_host.get_untracked(),
                        "diagnostics": ui.diagnostics.get_untracked(),
                        "logs": ui.diagnostics_logs.get_untracked(),
                    });
                    if let Ok(pretty) = serde_json::to_string_pretty(&report) {
                        copy_text(ui, pretty);
                    }
                },
            )),
    )
    .child(dynamic(move || {
        let host_id = ui.settings_host.get();
        let hosts = crate::hosts::effective_hosts(ui);
        let summary = hosts
            .iter()
            .find(|h| h.id == host_id)
            .cloned()
            .or_else(|| hosts.into_iter().next());
        let runtime = crate::config_for_host(&host_id);
        let diag = ui.diagnostics.get();

        let mut stack = view().gap_4().min_w(px(0.));

        // Top health pills
        let mut pills = view().flex_row().flex_wrap().items_center().gap_2();
        if let Some(h) = &summary {
            let conn_pill = match &h.connection {
                HostConnectionState::Connected { rtt_ms, api_version, .. } => {
                    let lat = if h.is_local {
                        "Socket · 0ms".to_owned()
                    } else {
                        format!("{rtt_ms}ms")
                    };
                    live_status(format!("{lat} · API v{api_version}"), Tone::Success)
                }
                _ => crate::hosts::host_status_badge(h),
            };
            pills = pills.child(conn_pill);
        }

        if let Some(err) = diag["error"].as_str() {
            stack = stack.child(pills);
            let mut conn_group = settings_group().title("Connection & Herdr Configuration");
            if let Some(h) = &summary {
                conn_group = conn_group
                    .row(settings_kv_row("Host", mono_val(h.name.clone())))
                    .row(settings_kv_row("Transport", mono_val(h.transport_label.clone())))
                    .row(settings_kv_row("Endpoint", mono_val(h.endpoint_label.clone())))
                    .row(settings_kv_row("Status", mono_val(h.connection.short_status())));
            }
            conn_group = conn_group
                .row(settings_kv_row("Herdr executable", mono_val(runtime.hc)))
                .row(settings_kv_row("Herdr session", mono_val(runtime.hc_session)));
            stack = stack.child(conn_group);

            let hid_rec = host_id.clone();
            let is_updating = summary.as_ref().is_some_and(HostSummary::is_updating);
            stack = stack.child(
                view()
                    .p_3p5()
                    .gap_2()
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(if is_updating { ACCENT } else { ATTENTION })
                    .bg(if is_updating { SELECTED } else { ATTENTION_SURFACE })
                    .child(
                        label(if is_updating {
                            "Remote Daemon Updating"
                        } else {
                            "Remote Diagnostics Unreachable"
                        })
                        .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(muted(err.to_owned()))
                    .child(
                        view().items_start().child(if is_updating {
                            compact_action("Updating…", Variant::Secondary, |_| {}).disabled(|| true)
                        } else {
                            compact_action(
                                "Reconnect Host",
                                Variant::Secondary,
                                move |_| crate::hosts::reconnect_host(ui, Some(hid_rec.clone())),
                            )
                        }),
                    ),
            );
            return stack;
        }

        if diag.is_null() {
            stack = stack.child(pills).child(loading_panel(
                "Running host diagnostics…",
                "Inspecting AppServer & SQLite WAL, Herdr Core supervision, toolchain binaries, and daemon logs.",
            ));
            return stack;
        }

        let srv = &diag["server"];
        let stg = &diag["storage"];
        let db_ok = stg["healthy"].as_bool().unwrap_or(false);
        let db_bytes = stg["db_size_bytes"].as_u64().unwrap_or(0);
        let herdr = &diag["herdr"];
        let hc_ready = herdr["check"]["ready"].as_bool().unwrap_or(false);
        let obs_conn = herdr["observer_connected"].as_bool().unwrap_or(false);
        let obs_ptys = herdr["observed_terminals"].as_u64().unwrap_or(0);
        let herdr_ok = hc_ready && obs_conn;
        let tc = &diag["toolchain"];
        let git_ok = tc["git"]["available"].as_bool().unwrap_or(false);
        let py_ok = tc["python3"]["available"].as_bool().unwrap_or(false);

        pills = pills
            .child(status(
                format!(
                    "SQLite {} · {}",
                    stg["journal_mode"].as_str().unwrap_or("wal").to_uppercase(),
                    format_bytes(db_bytes)
                ),
                if db_ok { Tone::Success } else { Tone::Danger },
            ))
            .child(status(
                if obs_conn {
                    format!("Herdr · {obs_ptys} PTYs")
                } else if hc_ready {
                    "Herdr · Ready".to_owned()
                } else {
                    "Herdr · Unavailable".to_owned()
                },
                if herdr_ok || hc_ready {
                    Tone::Success
                } else {
                    Tone::Danger
                },
            ))
            .child(status(
                if git_ok && py_ok {
                    "Toolchain · Ready"
                } else {
                    "Toolchain · Missing tools"
                },
                if git_ok && py_ok {
                    Tone::Success
                } else {
                    Tone::Danger
                },
            ));
        stack = stack.child(pills);

        // Group 1: Connection, Service & Herdr Runtime
        let mut runtime_group = settings_group().title("Connection & Runtime");
        if let Some(h) = &summary {
            runtime_group = runtime_group
                .row(settings_kv_row("Host", mono_val(h.name.clone())))
                .row(settings_kv_row(
                    "Transport",
                    mono_val(format!("{} · {}", h.transport_label, h.endpoint_label)),
                ));
            if let Some(fp) = &h.server_pubkey_fingerprint {
                runtime_group = runtime_group.row(settings_kv_row("Key fingerprint", mono_val(fp.clone())));
            }
            match &h.connection {
                HostConnectionState::Connected {
                    rtt_ms,
                    api_version,
                    version,
                    platform,
                } => {
                    runtime_group = runtime_group
                        .row(settings_kv_row(
                            "Latency & API",
                            mono_val(format!(
                                "{} · API v{api_version} (Desktop v{})",
                                if h.is_local {
                                    "0ms (local)".to_owned()
                                } else {
                                    format!("{rtt_ms}ms")
                                },
                                crate::backend::desktop_api()
                            )),
                        ))
                        .row(settings_kv_row("Daemon build", mono_val(format!("{version} ({platform})"))));
                }
                HostConnectionState::Updating { version } => {
                    let msg = if version.trim().is_empty() {
                        "Updating…".to_owned()
                    } else {
                        format!("Updating to {version}…")
                    };
                    runtime_group = runtime_group.row(settings_kv_row("Daemon status", mono_val(msg)));
                }
                HostConnectionState::ApiMismatch {
                    server_api,
                    app_api,
                    version,
                } => {
                    let h_upd = h.clone();
                    runtime_group = runtime_group.row(settings_kv_row(
                        "API mismatch",
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(mono_val(format!(
                                "Daemon v{server_api} · Desktop v{app_api} ({version})"
                            )))
                            .child(compact_action(
                                "Update Daemon",
                                Variant::Primary,
                                move |_| crate::hosts::update_host_daemon(ui, h_upd.clone()),
                            )),
                    ));
                }
                _ => {
                    runtime_group =
                        runtime_group.row(settings_kv_row("Status", mono_val(h.connection.short_status())));
                }
            }
            if !h.is_local {
                runtime_group =
                    runtime_group.row(settings_kv_row("Reconnect count", mono_val(h.reconnect_count.to_string())));
            }
            if let Some(err) = h.connection.error_detail() {
                runtime_group = runtime_group.row(
                    view()
                        .px_3p5()
                        .py_2()
                        .child(text(err.to_owned()).text_size(px(12.)).text_color(ATTENTION)),
                );
            }
        }

        let bound_tcp = srv["bound_tcp"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "None (Unix socket only)".into());

        runtime_group = runtime_group
            .row(settings_kv_row(
                "AppServer",
                mono_val(format!(
                    "{} (pid {}) · up {}",
                    srv["hostname"].as_str().unwrap_or("unknown"),
                    srv["pid"].as_u64().unwrap_or(0),
                    format_uptime(srv["uptime_ms"].as_u64().unwrap_or(0))
                )),
            ))
            .row(settings_kv_row(
                "Application home",
                mono_val(srv["home"].as_str().unwrap_or("").to_owned()),
            ))
            .row(settings_kv_row("TCP listener", mono_val(bound_tcp)))
            .row(settings_kv_row(
                "SQLite WAL",
                mono_val(format!(
                    "{} · DB {} · WAL {}",
                    stg["journal_mode"].as_str().unwrap_or("wal").to_uppercase(),
                    format_bytes(db_bytes),
                    format_bytes(stg["wal_size_bytes"].as_u64().unwrap_or(0)),
                )),
            ))
            .row(settings_kv_row(
                "Entities",
                mono_val(format!(
                    "{} projects · {} active cards · {} live terminals",
                    stg["project_count"].as_u64().unwrap_or(0),
                    stg["active_card_count"].as_u64().unwrap_or(0),
                    stg["active_terminal_count"].as_u64().unwrap_or(0),
                )),
            ))
            .row(settings_kv_row(
                "Herdr runtime",
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(mono_val(format!(
                        "{} · session `{}` · core {}",
                        runtime.hc,
                        herdr["session"].as_str().unwrap_or(&runtime.hc_session),
                        herdr["core_id"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("unverified"),
                    )))
                    .child(compact_action(
                        "Check connection",
                        Variant::Secondary,
                        move |_| dispatch(ui, "herdr.status", json!({})),
                    )),
            ))
            .row(settings_kv_row(
                "Observer stream",
                mono_val(if obs_conn {
                    format!("Connected ({obs_ptys} observed PTYs)")
                } else {
                    "Reconnecting / idle".into()
                }),
            ));
        stack = stack.child(runtime_group);

        // Group 2: Toolchain & Agent Binaries
        let ts_ip = tc["tailscale"]["ipv4"].as_str().unwrap_or("Not connected");
        let svc_mgr = &tc["service_manager"];
        let svc_summary = format!(
            "{} · {}{}",
            svc_mgr["kind"].as_str().unwrap_or("manual"),
            if svc_mgr["active"].as_bool().unwrap_or(false) {
                "active"
            } else if svc_mgr["installed"].as_bool().unwrap_or(false) {
                "installed (inactive)"
            } else {
                "not installed"
            },
            match svc_mgr["linger"].as_bool() {
                Some(true) => " · linger enabled",
                Some(false) => " · linger disabled",
                None => "",
            }
        );

        let mut tc_group = settings_group()
            .title("Toolchain & Enabled Agent Binaries")
            .row(settings_kv_row(
                "git",
                mono_val(
                    tc["git"]["version"]
                        .as_str()
                        .or_else(|| tc["git"]["error"].as_str())
                        .unwrap_or("missing")
                        .to_owned(),
                ),
            ))
            .row(settings_kv_row(
                "python3",
                mono_val(
                    tc["python3"]["version"]
                        .as_str()
                        .or_else(|| tc["python3"]["error"].as_str())
                        .unwrap_or("missing")
                        .to_owned(),
                ),
            ))
            .row(settings_kv_row("Tailscale IPv4", mono_val(ts_ip.to_owned())))
            .row(settings_kv_row("Background service", mono_val(svc_summary)));

        if let Some(profs) = tc["profiles"].as_array() {
            for p in profs {
                let pid = p["id"].as_str().unwrap_or("");
                let cmd = p["command"].as_str().unwrap_or("");
                let ok = p["available"].as_bool().unwrap_or(false);
                let resolved = p["resolved_path"].as_str().unwrap_or("missing in PATH");
                tc_group = tc_group.row(
                    view()
                        .px_3p5()
                        .py_2()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .min_w(px(0.))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .flex_shrink_0()
                                .child(live_dot(if ok { SUCCESS } else { DANGER }))
                                .child(label(format!("@{pid} ({cmd})")).text_size(px(12.))),
                        )
                        .child(
                            muted(resolved.to_owned())
                                .font_family(MONO_FONT)
                                .text_size(px(11.5))
                                .truncate(),
                        ),
                );
            }
        }
        stack = stack.child(tc_group);

        // Group 3: Live Log Tail (borderless header + single-border log box)
        let log_val = ui.diagnostics_logs.get();
        let cur_src = {
            let s = ui.diagnostics_log_source.get();
            if s.is_empty() { "server".to_owned() } else { s }
        };
        let log_text = log_val["text"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("No log entries recorded yet.")
            .to_owned();
        let log_text_copy = log_text.clone();
        let src_server = cur_src.clone();
        let src_updater = cur_src;

        let log_header = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(label("Daemon Logs").font_weight(FontWeight::SEMIBOLD))
                    .child(
                        segmented()
                            .child(compact_segment(
                                "server.log",
                                None,
                                move || src_server == "server",
                                move |_| {
                                    ui.diagnostics_log_source.set("server".into());
                                    refresh_diagnostics(ui);
                                },
                            ))
                            .child(compact_segment(
                                "updater.log",
                                None,
                                move || src_updater == "updater",
                                move |_| {
                                    ui.diagnostics_log_source.set("updater".into());
                                    refresh_diagnostics(ui);
                                },
                            )),
                    ),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .child(compact_action("Refresh", Variant::Quiet, move |_| {
                        refresh_diagnostics(ui);
                    }))
                    .child(compact_action("Copy Log", Variant::Quiet, move |_| {
                        copy_text(ui, log_text_copy.clone());
                    })),
            );

        stack = stack.child(
            view()
                .gap_2()
                .min_w(px(0.))
                .child(log_header)
                .child(
                    view()
                        .rounded(px(RADIUS))
                        .border_1()
                        .border_color(BORDER)
                        .bg(SURFACE)
                        .overflow_hidden()
                        .min_w(px(0.))
                        .child(
                            scroll()
                                .max_h(px(260.))
                                .p_3()
                                .child(
                                    text(log_text)
                                        .font_family(MONO_FONT)
                                        .text_size(px(11.5))
                                        .text_color(MUTED)
                                        .min_w(px(0.)),
                                ),
                        ),
                ),
        );

        stack
    }))
}
