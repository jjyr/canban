//! Multi-host `canband` connection chrome, `Connect a Machine` discovery/pairing wizard,
//! `Settings -> Hosts` management surface, `+ New Project` host selector, and offline banner.
use crate::presenter::*;
use canban_types::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;
use std::time::Duration;

fn sanitize_host_id(raw: &str) -> String {
    let mut slug: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let clean = slug.trim_matches('-').to_owned();
    let base = if clean.is_empty() || clean == "local" {
        "host".to_owned()
    } else {
        clean
    };
    format!("{base}-{}", &canban_types::id()[..6])
}

/// Effective list of `HostSummary` rows (`This Mac` first, followed by saved remote hosts),
/// supporting both live `HostRegistry` sessions and deterministic Gallery fixtures.
pub fn effective_hosts(ui: Ui) -> Vec<HostSummary> {
    let direct = ui.hosts.get();
    if !direct.is_empty() {
        return direct;
    }
    let from_snapshot: Vec<HostSummary> = ui.snapshot.with(|s| {
        let mut list: Vec<HostSummary> = s.host_summaries.values().cloned().collect();
        list.sort_by(|a, b| {
            b.is_local
                .cmp(&a.is_local)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        list
    });
    if !from_snapshot.is_empty() {
        return from_snapshot;
    }
    let active = ui.snapshot.with(canban_scheduler::active_count);
    let projects = ui.snapshot.with(|s| s.projects.len());
    vec![HostSummary {
        id: "local".into(),
        name: "This Mac".into(),
        is_local: true,
        transport_label: "Local socket".into(),
        endpoint_label: "~/.canban/app.sock".into(),
        ssh_target: None,
        server_id: None,
        connection: HostConnectionState::Connected {
            rtt_ms: 0,
            api_version: canban_updater::DESKTOP_API,
            version: env!("CARGO_PKG_VERSION").into(),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        },
        active_agents: active,
        max_agents: ui.max_agents.get(),
        project_count: projects,
        reconnect_count: 0,
        last_synced_at: None,
    }]
}

/// Human-readable display name for a `canband` host (`"This Mac"` for `"local"`).
pub fn host_name(ui: Ui, host_id: &str) -> String {
    if host_id.is_empty() || host_id == "local" {
        return "This Mac".into();
    }
    if let Some(found) = ui
        .hosts
        .with(|hs| hs.iter().find(|h| h.id == host_id).map(|h| h.name.clone()))
    {
        return found;
    }
    ui.snapshot
        .with(|s| s.host_display_name(host_id).to_owned())
}

pub fn toggle_popover(ui: Ui) {
    crate::utilities::open(ui, "hosts");
}

pub fn open_connect_modal(ui: Ui) {
    refresh_discovery(ui);
    crate::utilities::open(ui, "connect-host");
}

pub fn close_overlays(ui: Ui) {
    let cur = ui.utility_popup.get_untracked();
    if cur == "hosts" || cur == "connect-host" {
        crate::utilities::close(ui);
    }
}

pub fn refresh_discovery(ui: Ui) {
    if ui.discovery_loading.get_untracked() || crate::APP.get().is_none() {
        return;
    }
    ui.discovery_loading.set(true);
    ui.spawn(async move {
        let res = blocking(canban_app_server::network::discover_all).await;
        let _ = ui.discovery_loading.try_set(false);
        if let Ok(cat) = res {
            let _ = ui.discovery.try_set(cat);
        }
    });
}

pub fn reconnect_host(ui: Ui, host_id: Option<String>) {
    if let Some(reg) = crate::HOSTS.get().cloned() {
        ui.spawn(async move {
            let target = host_id.clone();
            let _ = blocking(move || {
                reg.reconnect(target.as_deref());
                reg.poll_remotes_once();
                (reg.summaries(), reg.merged_snapshot())
            })
            .await
            .map(|(sums, snap_res)| {
                let _ = ui.hosts.try_set(sums);
                if let Ok(snap) = snap_res {
                    let _ = ui.snapshot.try_set(snap);
                }
            });
            ui.notify(match host_id {
                Some(_) => "Reconnecting host…",
                None => "Reconnecting remote hosts…",
            });
        });
    } else {
        ui.notify("Gallery preview: simulated host reconnect");
    }
}

fn infer_ssh_target_for_host(summary: &HostSummary) -> Option<String> {
    if let Some(t) = summary
        .ssh_target
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(t.to_owned());
    }
    let host_part = summary
        .endpoint_label
        .split(':')
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "~/.canban/app.sock")?;
    let user = std::env::var("USER").unwrap_or_default();
    if user.trim().is_empty() || host_part.contains('@') {
        Some(host_part.to_owned())
    } else {
        Some(format!("{}@{host_part}", user.trim()))
    }
}

pub fn update_host_daemon(ui: Ui, summary: HostSummary) {
    let host_id = summary.id.clone();
    let host_name = summary.name.clone();
    let ssh_fallback = infer_ssh_target_for_host(&summary);
    let prefer_tailscale = !summary.transport_label.contains("SSH");
    if let Some(reg) = crate::HOSTS.get().cloned() {
        ui.notify(format!("Updating Canban daemon on {host_name}…"));
        ui.spawn(async move {
            let hid = host_id.clone();
            let res = blocking(move || {
                let rpc_res = reg.update_remote_daemon(&hid);
                let sums = reg.summaries();
                let snap = reg.merged_snapshot();
                (rpc_res, sums, snap)
            })
            .await;
            match res {
                Ok((Ok(val), sums, snap_res)) => {
                    let _ = ui.hosts.try_set(sums);
                    if let Ok(snap) = snap_res {
                        let _ = ui.snapshot.try_set(snap);
                    }
                    if val["updated"] == true {
                        let ver = val["version"].as_str().unwrap_or("latest");
                        ui.notify(format!("Updated {host_name} daemon to {ver}"));
                    } else if let Some(msg) = val["message"].as_str() {
                        ui.operation_error.set(msg.to_owned());
                    }
                }
                Ok((Err(err), sums, snap_res)) => {
                    let _ = ui.hosts.try_set(sums);
                    if let Ok(snap) = snap_res {
                        let _ = ui.snapshot.try_set(snap);
                    }
                    let msg = format!("{err:#}");
                    if msg.contains("Unknown update operation: update.daemon")
                        && let Some(ssh_target) = ssh_fallback
                    {
                        spawn_ssh_bootstrap_terminal(
                            ui,
                            &ssh_target,
                            prefer_tailscale,
                            DEFAULT_DAEMON_PORT,
                        );
                    } else {
                        ui.operation_error
                            .set(format!("Could not update daemon on {host_name}: {msg}"));
                    }
                }
                Err(e) => {
                    ui.operation_error.set(e.to_string());
                }
            }
        });
    } else {
        ui.notify(format!(
            "Gallery preview: simulated daemon update on {host_name}"
        ));
    }
}

pub fn open_host_terminal(ui: Ui, host_id: String) {
    close_overlays(ui);
    crate::terminals::launch_custom(ui, json!({ "host_id": host_id }));
}

pub fn open_host_settings(ui: Ui, host_id: String, section: &'static str) {
    close_overlays(ui);
    ui.settings_host.set(host_id);
    ui.settings_section.set(section.into());
    ui.navigate("settings");
}

fn spawn_ssh_bootstrap_terminal(ui: Ui, ssh_target: &str, prefer_tailscale: bool, port: u16) {
    let repo = std::env::var("CANBAN_UPDATE_REPOSITORY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "jjyr/canban".into());
    let script = canban_app_server::network::build_ssh_bootstrap_command(
        ssh_target,
        &repo,
        prefer_tailscale,
        port,
    );
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let title = ui
        .snapshot
        .with_untracked(|s| s.next_unique_terminal_title("Add machine"));
    close_overlays(ui);
    crate::terminals::launch_custom(
        ui,
        json!({
            "host_id": "local",
            "name": title,
            "setup_argv": [shell, "-lc", script],
        }),
    );
    ui.notify(format!(
        "Opened interactive setup terminal for {}",
        ssh_target.trim()
    ));
}

pub(crate) fn status_tone(summary: &HostSummary) -> Tone {
    if summary.is_connected() {
        Tone::Success
    } else if summary.needs_attention() {
        Tone::Danger
    } else {
        Tone::Attention
    }
}

pub(crate) fn dot_color_for_tone(tone: Tone) -> ThemeColor {
    match tone {
        Tone::Success => SUCCESS,
        Tone::Danger => DANGER,
        Tone::Attention => ATTENTION,
        Tone::Info => ACCENT,
        Tone::Neutral => MUTED,
    }
}

/// Top-chrome status pill mounted next to `Activity & Notifications`.
pub fn trigger_pill(ui: Ui) -> impl IntoElement {
    let hosts_memo = computed(move || effective_hosts(ui));
    let is_open = computed(move || {
        let p = ui.utility_popup.get();
        p == "hosts" || p == "connect-host"
    });

    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(
            hint_tooltip(move || {
                let list = hosts_memo.get();
                let remote_count = list.iter().filter(|h| !h.is_local).count();
                if remote_count == 0 {
                    "Connected Hosts\nThis Mac (local) · Click to connect a remote machine".into()
                } else {
                    let online = list
                        .iter()
                        .filter(|h| !h.is_local && h.is_connected())
                        .count();
                    format!(
                        "Connected Hosts\nThis Mac + {} / {} remote online",
                        online, remote_count
                    )
                }
            })
            .placement(Placement::Bottom),
        )
        .child(dynamic(move || {
            let list = hosts_memo.get();
            let remote_count = list.iter().filter(|h| !h.is_local).count();
            let any_attention = list.iter().any(|h| !h.is_local && h.needs_attention());
            let any_offline = list.iter().any(|h| !h.is_local && h.is_offline());
            let online_remotes = list
                .iter()
                .filter(|h| !h.is_local && h.is_connected())
                .count();
            let open = is_open.get();

            let border = if open {
                ACCENT
            } else if any_attention {
                DANGER
            } else if any_offline {
                ATTENTION
            } else {
                BORDER
            };
            let bg = if open {
                SELECTED
            } else if any_attention || any_offline {
                ATTENTION_SURFACE
            } else {
                INSET
            };
            let dot_color = if any_attention {
                DANGER
            } else if any_offline {
                ATTENTION
            } else if remote_count > 0 {
                SUCCESS
            } else {
                MUTED
            };

            let mut btn = press_target()
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
                        if remote_count == 0 {
                            "Connected Hosts, This Mac only".to_owned()
                        } else {
                            format!(
                                "Connected Hosts, {} of {} remote online",
                                online_remotes, remote_count
                            )
                        }
                        .into(),
                    )
                })
                .on_press(move |_: &()| toggle_popover(ui));

            btn = btn.child(
                icon_tinted(
                    Icon::Server,
                    if open {
                        ACCENT
                    } else if any_attention {
                        DANGER
                    } else if any_offline {
                        ATTENTION
                    } else {
                        MUTED
                    },
                )
                .w(px(13.5))
                .h(px(13.5)),
            );

            if remote_count > 0 {
                btn = btn
                    .child(view().w(px(6.)).h(px(6.)).rounded_full().bg(dot_color))
                    .child(
                        muted(format!("{}/{}", online_remotes + 1, remote_count + 1))
                            .text_size(px(11.5))
                            .font_family(MONO_FONT),
                    );
            }
            btn
        }))
}

/// Top-right anchored `Hosts` quick popover (`popup == "hosts"`).
pub fn popover(ui: Ui) -> impl IntoElement {
    nsh_base::dialog()
        .animation(canban_ui_kit::motion::modal)
        .close_on_escape(true)
        .close_on_backdrop_press(true)
        .on_cancel(move || close_overlays(ui))
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
                        .label("Dismiss Connected Hosts")
                        .on_press(move || close_overlays(ui)),
                )
                .child(
                    panel()
                        .w(px(460.))
                        .max_w(Percentage(0.95))
                        .p_0()
                        .gap_0()
                        .overflow_hidden()
                        .border_1()
                        .border_color(BORDER)
                        .bg(SURFACE)
                        .child(
                            view()
                                .px_3p5()
                                .py_2p5()
                                .border_b_1()
                                .border_color(BORDER)
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(icon(Icon::Server).w(px(15.)).h(px(15.)))
                                        .child(
                                            label("Connected Hosts")
                                                .font_weight(FontWeight::SEMIBOLD),
                                        ),
                                )
                                .child(compact_action(
                                    "+ Connect Machine…",
                                    Variant::Primary,
                                    move |_| open_connect_modal(ui),
                                )),
                        )
                        .child(scroll().max_h(px(420.)).child(dynamic(move || {
                            let list = effective_hosts(ui);
                            let mut col = view().gap_0().min_w(px(0.));
                            for (idx, h) in list.iter().enumerate() {
                                let hid_term = h.id.clone();
                                let hid_rec = h.id.clone();
                                let hid_set = h.id.clone();
                                let hid_diag = h.id.clone();
                                let tone = status_tone(h);
                                let short = h.connection.short_status();
                                let err_opt = h.connection.error_detail().map(str::to_owned);
                                let is_local = h.is_local;
                                let is_conn = h.is_connected();

                                let mut actions =
                                    view().flex_row().flex_wrap().items_center().gap_1();
                                if !is_local {
                                    if matches!(
                                        h.connection,
                                        HostConnectionState::ApiMismatch { .. }
                                    ) {
                                        let h_upd = h.clone();
                                        actions = actions.child(compact_action(
                                            "Update Daemon",
                                            Variant::Primary,
                                            move |_| update_host_daemon(ui, h_upd.clone()),
                                        ));
                                    }
                                    actions = actions.child(compact_action(
                                        "Reconnect",
                                        if is_conn {
                                            Variant::Quiet
                                        } else {
                                            Variant::Secondary
                                        },
                                        move |_| reconnect_host(ui, Some(hid_rec.clone())),
                                    ));
                                }
                                if is_conn {
                                    actions = actions.child(compact_action(
                                        "Terminal",
                                        Variant::Quiet,
                                        move |_| open_host_terminal(ui, hid_term.clone()),
                                    ));
                                }
                                actions = actions
                                    .child(compact_action("Settings", Variant::Quiet, move |_| {
                                        open_host_settings(ui, hid_set.clone(), "scheduling")
                                    }))
                                    .child(compact_action(
                                        "Diagnostics",
                                        Variant::Quiet,
                                        move |_| {
                                            open_host_settings(ui, hid_diag.clone(), "diagnostics")
                                        },
                                    ));

                                let mut row = view()
                                    .px_3p5()
                                    .py_3()
                                    .gap_1p5()
                                    .min_w(px(0.))
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
                                                    .child(live_dot(dot_color_for_tone(tone)))
                                                    .child(
                                                        label(h.name.clone())
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .truncate(),
                                                    )
                                                    .child(badge(h.transport_label.clone())),
                                            )
                                            .child(status(short, tone)),
                                    )
                                    .child(
                                        view()
                                            .flex_row()
                                            .flex_wrap()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                muted(format!(
                                                    "{} · {} project{} · {}/{} agents",
                                                    h.endpoint_label,
                                                    h.project_count,
                                                    if h.project_count == 1 { "" } else { "s" },
                                                    h.active_agents,
                                                    h.max_agents
                                                ))
                                                .text_size(px(11.5)),
                                            )
                                            .child(actions),
                                    );

                                if let Some(err) = err_opt {
                                    row = row.child(
                                        muted(err)
                                            .text_color(if h.needs_attention() {
                                                DANGER
                                            } else {
                                                ATTENTION
                                            })
                                            .text_size(px(11.5)),
                                    );
                                }
                                if idx + 1 < list.len() {
                                    row = row.border_b_1().border_color(BORDER);
                                }
                                col = col.child(row);
                            }
                            col
                        })))
                        .child(
                            view()
                                .px_3p5()
                                .py_2()
                                .bg(INSET)
                                .border_t_1()
                                .border_color(BORDER)
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(compact_action(
                                    "Manage Hosts…",
                                    Variant::Quiet,
                                    move |_| open_host_settings(ui, "local".into(), "hosts"),
                                ))
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_1p5()
                                        .child(compact_action(
                                            "Reconnect All",
                                            Variant::Quiet,
                                            move |_| reconnect_host(ui, None),
                                        ))
                                        .child(compact_action(
                                            "Diagnostics →",
                                            Variant::Quiet,
                                            move |_| {
                                                let target = ui.active_project_host();
                                                open_host_settings(ui, target, "diagnostics");
                                            },
                                        )),
                                ),
                        ),
                ),
        )
}

/// `+ New Project` host selector (`This Mac` vs connected remote `canband` hosts).
pub fn new_project_host_picker(
    ui: Ui,
    target_host: Signal<String>,
    directory: Signal<String>,
) -> impl IntoElement {
    dynamic(move || {
        let hosts = effective_hosts(ui);
        if hosts.len() <= 1 {
            return view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(muted("Creating Project on This Mac."))
                .child(compact_action(
                    "+ Connect Remote Machine…",
                    Variant::Quiet,
                    move |_| open_connect_modal(ui),
                ))
                .into_element();
        }
        let selected_id = target_host.get();
        view()
            .gap_2()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(label("Canban Host"))
                    .child(compact_action(
                        "+ Connect Machine…",
                        Variant::Quiet,
                        move |_| open_connect_modal(ui),
                    )),
            )
            .child(segmented().children(hosts.into_iter().map(|h| {
                let hid = h.id.clone();
                let hid_sel = h.id.clone();
                let cur_sel = selected_id.clone();
                let is_online = h.is_connected();
                let title = if is_online {
                    h.name.clone()
                } else {
                    format!("{} (offline)", h.name)
                };
                compact_segment(
                    title,
                    Some(if h.is_local {
                        Icon::Projects
                    } else {
                        Icon::Server
                    }),
                    move || cur_sel == hid_sel,
                    move |_| {
                        if !is_online {
                            ui.operation_error.set(
                                "Reconnect this remote host before creating a new Project on it."
                                    .into(),
                            );
                            return;
                        }
                        if target_host.get_untracked() != hid {
                            target_host.set(hid.clone());
                            directory.set(String::new());
                        }
                    },
                )
                .into_element()
            })))
            .into_element()
    })
}

/// Calm read-only banner shown at the top of `Board` / `Card` when viewing an offline cached remote Project.
pub fn offline_banner(ui: Ui) -> impl IntoElement {
    when(
        move || ui.is_active_project_offline(),
        move || {
            dynamic(move || {
                let host_id = ui.active_project_host();
                let (host_name, detail, mismatch_sum) = ui.snapshot.with(|s| {
                    let name = s.host_display_name(&host_id).to_owned();
                    let sum = s.host_summary(&host_id).cloned();
                    let status = sum
                        .as_ref()
                        .map(|h| h.connection.short_status())
                        .unwrap_or_else(|| "Offline · Cached".into());
                    let mm = sum.filter(|h| {
                        matches!(h.connection, HostConnectionState::ApiMismatch { .. })
                    });
                    (name, status, mm)
                });
                let hid_rec = host_id.clone();
                let hid_diag = host_id;
                let mut trailing = view().flex_row().items_center().gap_1p5();
                if let Some(h_upd) = mismatch_sum {
                    trailing = trailing.child(compact_action(
                        "Update Daemon",
                        Variant::Primary,
                        move |_| update_host_daemon(ui, h_upd.clone()),
                    ));
                }
                trailing = trailing
                    .child(compact_action("Reconnect", Variant::Secondary, move |_| {
                        reconnect_host(ui, Some(hid_rec.clone()));
                    }))
                    .child(compact_action("Diagnostics", Variant::Quiet, move |_| {
                        open_host_settings(ui, hid_diag.clone(), "diagnostics");
                    }));
                view()
                    .px_4()
                    .py_2()
                    .bg(ATTENTION_SURFACE)
                    .border_b_1()
                    .border_color(ATTENTION)
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
                            .min_w(px(0.))
                            .child(live_dot(ATTENTION))
                            .child(
                                label(format!(
                                    "{host_name} is offline ({detail}) — viewing last cached Kanban state read-only while remote Agents continue on the host."
                                ))
                                .text_size(px(12.5)),
                            ),
                    )
                    .child(trailing)
            })
        },
    )
}

fn finish_host_registration(
    ui: Ui,
    saved: SavedHost,
    busy: Signal<bool>,
    feedback: Signal<String>,
    feedback_tone: Signal<Tone>,
) {
    if crate::HOSTS.get().is_none() {
        let sum = HostSummary {
            id: saved.id.clone(),
            name: saved.name.clone(),
            is_local: false,
            transport_label: saved.transport.label().into(),
            endpoint_label: saved.transport.endpoint_display(),
            ssh_target: saved.ssh_target.clone(),
            server_id: Some(format!("srv-{}", saved.id)),
            connection: HostConnectionState::Connected {
                rtt_ms: 14,
                api_version: canban_updater::DESKTOP_API,
                version: env!("CARGO_PKG_VERSION").into(),
                platform: "linux-x86_64".into(),
            },
            active_agents: 0,
            max_agents: 8,
            project_count: 0,
            reconnect_count: 0,
            last_synced_at: Some(canban_types::now()),
        };
        ui.hosts.update(|list| {
            if list.is_empty() {
                *list = effective_hosts(ui);
            }
            list.push(sum.clone());
        });
        ui.snapshot.update(|s| {
            s.host_summaries.insert(sum.id.clone(), sum);
        });
        close_overlays(ui);
        ui.notify(format!("Connected to {}", saved.name));
        return;
    }
    let reg = crate::HOSTS.get().unwrap().clone();
    busy.set(true);
    feedback.set(format!("Connecting to {}…", saved.name));
    feedback_tone.set(Tone::Neutral);
    ui.spawn(async move {
        let host_name = saved.name.clone();
        let res = blocking(move || {
            let sum = reg.add_host(saved)?;
            let snap = reg.merged_snapshot()?;
            let all = reg.summaries();
            Ok::<_, anyhow::Error>((sum, snap, all))
        })
        .await;
        let _ = busy.try_set(false);
        match res {
            Ok(Ok((_sum, snap, all))) => {
                let _ = ui.hosts.try_set(all);
                let _ = ui.snapshot.try_set(snap);
                close_overlays(ui);
                ui.notify(format!("Connected to {host_name}"));
            }
            Ok(Err(e)) => {
                let _ = feedback_tone.try_set(Tone::Danger);
                let _ = feedback.try_set(format!("{e:#}"));
            }
            Err(e) => {
                let _ = feedback_tone.try_set(Tone::Danger);
                let _ = feedback.try_set(e.to_string());
            }
        }
    });
}

/// Modal wizard for discovering, bootstrapping, and pairing a remote `canband` (`popup == "connect-host"`).
pub fn connect_modal(ui: Ui) -> impl IntoElement {
    let active_tab = signal("tailscale".to_owned());
    let busy = signal(false);
    let feedback = signal(String::new());
    let feedback_tone = signal(Tone::Neutral);

    let selected_ts_ip = signal(String::new());
    let selected_ts_name = signal(String::new());
    let ts_ssh_user = signal(std::env::var("USER").unwrap_or_else(|_| "root".into()));
    let ts_manual_uri = signal(String::new());
    let ts_needs_bootstrap = signal(false);

    let ssh_target = signal(String::new());
    let ssh_display_name = signal(String::new());
    let ssh_prefer_tailscale = signal(true);
    let ssh_needs_bootstrap = signal(false);

    let uri_input = signal(String::new());
    let uri_display_name = signal(String::new());

    if ui
        .discovery
        .with_untracked(|d| d.tailscale_peers.is_empty() && d.ssh_hosts.is_empty())
    {
        refresh_discovery(ui);
    }

    nsh_base::dialog()
        .animation(canban_ui_kit::motion::modal)
        .close_on_escape(true)
        .close_on_backdrop_press(true)
        .on_cancel(move || close_overlays(ui))
        .child(
            view()
                .flex_1()
                .items_center()
                .justify_center()
                .p_6()
                .bg(rgba(0x00000055))
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .label("Close Connect a Machine")
                        .on_press(move || close_overlays(ui)),
                )
                .child(
                    panel()
                        .w(px(620.))
                        .max_w(Percentage(0.95))
                        .max_h(Percentage(0.88))
                        .p_0()
                        .gap_0()
                        .overflow_hidden()
                        .border_1()
                        .border_color(BORDER)
                        .bg(SURFACE)
                        .child(
                            view()
                                .px_4()
                                .py_3()
                                .border_b_1()
                                .border_color(BORDER)
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    view()
                                        .gap_0p5()
                                        .child(
                                            label("Connect a Machine")
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_size(px(15.)),
                                        )
                                        .child(muted(
                                            "Run Canban + Herdr autonomously on a remote Linux or macOS host; data streams directly over Tailscale whenever reachable.",
                                        )),
                                )
                                .child(chrome_button(Icon::Close, "Close", move |_| {
                                    close_overlays(ui)
                                })),
                        )
                        .child(
                            view()
                                .px_4()
                                .py_2p5()
                                .bg(INSET)
                                .border_b_1()
                                .border_color(BORDER)
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    segmented()
                                        .child(compact_segment(
                                            "Tailscale",
                                            Some(Icon::Server),
                                            move || active_tab.get() == "tailscale",
                                            move |_| {
                                                feedback.set(String::new());
                                                active_tab.set("tailscale".into());
                                            },
                                        ))
                                        .child(compact_segment(
                                            "SSH Host",
                                            Some(Icon::Terminal),
                                            move || active_tab.get() == "ssh",
                                            move |_| {
                                                feedback.set(String::new());
                                                active_tab.set("ssh".into());
                                            },
                                        ))
                                        .child(compact_segment(
                                            "Pairing URI",
                                            Some(Icon::Copy),
                                            move || active_tab.get() == "uri",
                                            move |_| {
                                                feedback.set(String::new());
                                                active_tab.set("uri".into());
                                            },
                                        )),
                                )
                                .child(compact_action(
                                    move || {
                                        if ui.discovery_loading.get() {
                                            "Scanning…".to_owned()
                                        } else {
                                            "Refresh".to_owned()
                                        }
                                    },
                                    Variant::Quiet,
                                    move |_| refresh_discovery(ui),
                                )),
                        )
                        .child(
                            scroll()
                                .p_4()
                                .max_h(px(460.))
                                .child(dynamic(move || {
                                    let tab = active_tab.get();
                                    let mut body = view().gap_3().min_w(px(0.));

                                    if !feedback.get().is_empty() {
                                        body = body.child(notice(
                                            move || feedback.get(),
                                            feedback_tone.get(),
                                        ));
                                    }

                                    if tab == "tailscale" {
                                        body = body.child(connect_tailscale_tab(
                                            ui,
                                            busy,
                                            feedback,
                                            feedback_tone,
                                            selected_ts_ip,
                                            selected_ts_name,
                                            ts_ssh_user,
                                            ts_manual_uri,
                                            ts_needs_bootstrap,
                                        ));
                                    } else if tab == "ssh" {
                                        body = body.child(connect_ssh_tab(
                                            ui,
                                            busy,
                                            feedback,
                                            feedback_tone,
                                            ssh_target,
                                            ssh_display_name,
                                            ssh_prefer_tailscale,
                                            ssh_needs_bootstrap,
                                        ));
                                    } else {
                                        body = body.child(connect_uri_tab(
                                            ui,
                                            busy,
                                            feedback,
                                            feedback_tone,
                                            uri_input,
                                            uri_display_name,
                                        ));
                                    }

                                    body
                                })),
                        ),
                ),
        )
}

#[allow(clippy::too_many_arguments)]
fn connect_tailscale_tab(
    ui: Ui,
    busy: Signal<bool>,
    feedback: Signal<String>,
    feedback_tone: Signal<Tone>,
    selected_ts_ip: Signal<String>,
    selected_ts_name: Signal<String>,
    ts_ssh_user: Signal<String>,
    ts_manual_uri: Signal<String>,
    ts_needs_bootstrap: Signal<bool>,
) -> View {
    let mut body = view().gap_3().min_w(px(0.));
    let cat = ui.discovery.get();
    if !cat.tailscale_available && cat.tailscale_peers.is_empty() {
        body = body.child(notice(
            cat.tailscale_error.unwrap_or_else(|| {
                "Tailscale CLI is not running on This Mac. Start Tailscale or connect via SSH / Pairing URI.".into()
            }),
            Tone::Attention,
        ));
    } else if cat.tailscale_peers.is_empty() {
        body = body.child(muted("No other Tailscale peers found in your tailnet."));
    } else {
        let mut peer_list = panel()
            .p_0()
            .gap_0()
            .border_1()
            .border_color(BORDER)
            .overflow_hidden();
        for (idx, peer) in cat.tailscale_peers.iter().enumerate() {
            let ip = peer.preferred_ip().unwrap_or("").to_owned();
            let host_label = peer.hostname.clone();
            let os_badge = peer.os.clone();
            let online = peer.online;
            let ip_for_btn = ip.clone();
            let name_for_btn = host_label.clone();

            let mut row = view()
                .px_3p5()
                .py_2p5()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .gap_0p5()
                        .min_w(px(0.))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(live_dot(if online {
                                    SUCCESS
                                } else {
                                    MUTED
                                }))
                                .child(label(host_label).font_weight(FontWeight::SEMIBOLD))
                                .child(badge(os_badge)),
                        )
                        .child(
                            muted(format!(
                                "{} · {}",
                                ip,
                                if peer.dns_name.is_empty() {
                                    if online { "online" } else { "offline" }
                                } else {
                                    peer.dns_name.as_str()
                                }
                            ))
                            .font_family(MONO_FONT)
                            .text_size(px(11.5)),
                        ),
                )
                .child(
                    compact_action(
                        "Pair & Connect",
                        if online {
                            Variant::Primary
                        } else {
                            Variant::Secondary
                        },
                        move |_| {
                            let peer_ip = ip_for_btn.clone();
                            let peer_name = name_for_btn.clone();
                            selected_ts_ip.set(peer_ip.clone());
                            selected_ts_name.set(peer_name.clone());
                            ts_needs_bootstrap.set(false);
                            if crate::APP.get().is_none() {
                                finish_host_registration(
                                    ui,
                                    SavedHost {
                                        id: sanitize_host_id(&peer_name),
                                        name: peer_name,
                                        transport: HostTransport::Tcp {
                                            addr: format!("{peer_ip}:{DEFAULT_DAEMON_PORT}"),
                                            token: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
                                        },
                                        ssh_target: None,
                                        server_id: None,
                                        enabled: true,
                                        added_at: canban_types::now(),
                                    },
                                    busy,
                                    feedback,
                                    feedback_tone,
                                );
                                return;
                            }
                            let ssh_usr = ts_ssh_user.get_untracked();
                            let ssh_spec = if ssh_usr.trim().is_empty() {
                                peer_ip.clone()
                            } else {
                                format!("{}@{}", ssh_usr.trim(), peer_ip)
                            };
                            busy.set(true);
                            feedback_tone.set(Tone::Neutral);
                            feedback.set(format!(
                                "Probing {peer_name} ({peer_ip}:{DEFAULT_DAEMON_PORT}) & attempting SSH auto-pair…"
                            ));
                            ui.spawn(async move {
                                let ip_c = peer_ip.clone();
                                let ssh_c = ssh_spec.clone();
                                let res = blocking(move || {
                                    let probe = canban_app_server::network::probe_tcp_host(
                                        &format!("{ip_c}:{DEFAULT_DAEMON_PORT}"),
                                        Duration::from_secs(2),
                                    );
                                    let ssh_pair =
                                        canban_app_server::network::pair_via_ssh_noninteractive(
                                            &ssh_c,
                                            true,
                                            DEFAULT_DAEMON_PORT,
                                        );
                                    (probe, ssh_pair)
                                })
                                .await;
                                let _ = busy.try_set(false);
                                if let Ok((probe_res, pair_res)) = res {
                                    match pair_res {
                                        Ok(paired) => {
                                            let token = paired["token"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .to_owned();
                                            let addr = paired["addr"]
                                                .as_str()
                                                .filter(|s| !s.is_empty())
                                                .map(str::to_owned)
                                                .unwrap_or_else(|| {
                                                    format!("{peer_ip}:{DEFAULT_DAEMON_PORT}")
                                                });
                                            finish_host_registration(
                                                ui,
                                                SavedHost {
                                                    id: sanitize_host_id(&peer_name),
                                                    name: peer_name,
                                                    transport: HostTransport::Tcp { addr, token },
                                                    ssh_target: Some(ssh_spec),
                                                    server_id: paired["server_id"]
                                                        .as_str()
                                                        .map(str::to_owned),
                                                    enabled: true,
                                                    added_at: canban_types::now(),
                                                },
                                                busy,
                                                feedback,
                                                feedback_tone,
                                            );
                                        }
                                        Err(pair_err) => {
                                            let msg = format!("{pair_err:#}");
                                            if probe_res.is_ok() {
                                                let _ = feedback_tone.try_set(Tone::Attention);
                                                let _ = feedback.try_set(format!(
                                                    "Canban daemon is running on {peer_name}:{DEFAULT_DAEMON_PORT}, automatic SSH key auth was skipped ({msg}). Paste `canban pair --tailscale` URI below or adjust SSH user."
                                                ));
                                            } else {
                                                let _ = ts_needs_bootstrap.try_set(true);
                                                let _ = feedback_tone.try_set(Tone::Attention);
                                                let _ = feedback.try_set(format!(
                                                    "Canban is not yet listening on {peer_name}:{DEFAULT_DAEMON_PORT}. Click 'Install & Pair in Terminal…' below for 1-click setup."
                                                ));
                                            }
                                        }
                                    }
                                }
                            });
                        },
                    )
                    .disabled(move || busy.get()),
                );
            if idx + 1 < cat.tailscale_peers.len() {
                row = row.border_b_1().border_color(BORDER);
            }
            peer_list = peer_list.child(row);
        }
        body = body.child(peer_list);
    }

    if !selected_ts_ip.get().is_empty() {
        let sel_ip = selected_ts_ip.get();
        let sel_name = selected_ts_name.get();
        body = body.child(
            panel()
                .p_3p5()
                .gap_2p5()
                .bg(INSET)
                .border_1()
                .border_color(BORDER)
                .child(
                    label(format!("Setup / Pair {sel_name} ({sel_ip})"))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_end()
                        .gap_2()
                        .child(
                            view()
                                .w(px(160.))
                                .gap_1()
                                .child(muted("SSH user").text_size(px(11.5)))
                                .child(field(ts_ssh_user, "ubuntu").label("SSH user")),
                        )
                        .child(compact_action(
                            "Install & Pair in Terminal…",
                            if ts_needs_bootstrap.get() {
                                Variant::Primary
                            } else {
                                Variant::Secondary
                            },
                            move |_| {
                                let usr = ts_ssh_user.get_untracked();
                                let ip = selected_ts_ip.get_untracked();
                                let spec = if usr.trim().is_empty() {
                                    ip
                                } else {
                                    format!("{}@{}", usr.trim(), ip)
                                };
                                spawn_ssh_bootstrap_terminal(ui, &spec, true, DEFAULT_DAEMON_PORT);
                            },
                        )),
                )
                .child(
                    view()
                        .gap_1()
                        .child(
                            muted("Or paste pairing URI from `canban pair --tailscale`:")
                                .text_size(px(11.5)),
                        )
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    view().flex_1().child(
                                        field(ts_manual_uri, "canban://100.x.y.z:7680#token")
                                            .label("Pairing URI"),
                                    ),
                                )
                                .child(compact_action(
                                    "Connect URI",
                                    Variant::Primary,
                                    move |_| {
                                        let raw = ts_manual_uri.get_untracked();
                                        let Some((addr, token)) = parse_pairing_uri(&raw) else {
                                            feedback_tone.set(Tone::Danger);
                                            feedback.set(
                                                "Enter a valid `canban://host:port#token` URI."
                                                    .into(),
                                            );
                                            return;
                                        };
                                        let nm = selected_ts_name.get_untracked();
                                        finish_host_registration(
                                            ui,
                                            SavedHost {
                                                id: sanitize_host_id(&nm),
                                                name: if nm.is_empty() { addr.clone() } else { nm },
                                                transport: HostTransport::Tcp { addr, token },
                                                ssh_target: None,
                                                server_id: None,
                                                enabled: true,
                                                added_at: canban_types::now(),
                                            },
                                            busy,
                                            feedback,
                                            feedback_tone,
                                        );
                                    },
                                )),
                        ),
                ),
        );
    }

    body
}

#[allow(clippy::too_many_arguments)]
fn connect_ssh_tab(
    ui: Ui,
    busy: Signal<bool>,
    feedback: Signal<String>,
    feedback_tone: Signal<Tone>,
    ssh_target: Signal<String>,
    ssh_display_name: Signal<String>,
    ssh_prefer_tailscale: Signal<bool>,
    ssh_needs_bootstrap: Signal<bool>,
) -> View {
    let mut body = view().gap_3().min_w(px(0.));
    let cat = ui.discovery.get();
    if !cat.ssh_hosts.is_empty() {
        let mut chips = view().flex_row().flex_wrap().gap_1p5();
        for entry in &cat.ssh_hosts {
            let spec = entry.target_spec();
            let alias = entry.alias.clone();
            chips = chips.child(compact_action(
                alias.clone(),
                if ssh_target.get() == spec {
                    Variant::Primary
                } else {
                    Variant::Secondary
                },
                move |_| {
                    ssh_target.set(spec.clone());
                    if ssh_display_name.get_untracked().trim().is_empty() {
                        ssh_display_name.set(alias.clone());
                    }
                },
            ));
        }
        body = body.child(
            view()
                .gap_1p5()
                .child(muted("Discovered in ~/.ssh/config:"))
                .child(chips),
        );
    }
    body.child(
        view()
            .gap_1p5()
            .child(label("SSH Target"))
            .child(field(ssh_target, "user@host or ~/.ssh/config alias").label("SSH target")),
    )
    .child(
        view()
            .gap_1p5()
            .child(label("Display Name"))
            .child(field(ssh_display_name, "Home Linux").label("Display name")),
    )
    .child(checkbox(
        "Prefer direct Tailscale TCP data transport when reachable (recommended)".to_owned(),
        ssh_prefer_tailscale,
        move |checked| ssh_prefer_tailscale.set(*checked),
    ))
    .child(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .pt_1()
            .child(compact_action(
                "Install & Pair in Terminal…",
                if ssh_needs_bootstrap.get() {
                    Variant::Primary
                } else {
                    Variant::Secondary
                },
                move |_| {
                    let target = ssh_target.get_untracked();
                    if target.trim().is_empty() {
                        feedback_tone.set(Tone::Danger);
                        feedback.set("Enter an SSH target first.".into());
                        return;
                    }
                    spawn_ssh_bootstrap_terminal(
                        ui,
                        &target,
                        ssh_prefer_tailscale.get_untracked(),
                        DEFAULT_DAEMON_PORT,
                    );
                },
            ))
            .child(
                action("Connect via SSH", Variant::Primary, move |_| {
                    let target = ssh_target.get_untracked().trim().to_owned();
                    if target.is_empty() {
                        feedback_tone.set(Tone::Danger);
                        feedback.set("Enter an SSH target such as `user@host`.".into());
                        return;
                    }
                    let disp = {
                        let raw = ssh_display_name.get_untracked();
                        if raw.trim().is_empty() {
                            target.split('@').next_back().unwrap_or(&target).to_owned()
                        } else {
                            raw.trim().to_owned()
                        }
                    };
                    let prefer_ts = ssh_prefer_tailscale.get_untracked();
                    if crate::APP.get().is_none() {
                        finish_host_registration(
                            ui,
                            SavedHost {
                                id: sanitize_host_id(&disp),
                                name: disp,
                                transport: HostTransport::Ssh {
                                    target: target.clone(),
                                    remote_home: None,
                                },
                                ssh_target: Some(target),
                                server_id: None,
                                enabled: true,
                                added_at: canban_types::now(),
                            },
                            busy,
                            feedback,
                            feedback_tone,
                        );
                        return;
                    }
                    busy.set(true);
                    ssh_needs_bootstrap.set(false);
                    feedback_tone.set(Tone::Neutral);
                    feedback.set(format!("Connecting to {target} over SSH…"));
                    ui.spawn(async move {
                        let t_clone = target.clone();
                        let pair_res = blocking(move || {
                            canban_app_server::network::pair_via_ssh_noninteractive(
                                &t_clone,
                                prefer_ts,
                                DEFAULT_DAEMON_PORT,
                            )
                        })
                        .await;
                        let _ = busy.try_set(false);
                        match pair_res {
                            Ok(Ok(paired)) => {
                                let ts_addr = paired["addr"]
                                    .as_str()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty() && prefer_ts)
                                    .map(str::to_owned);
                                let token = paired["token"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_owned();
                                let transport = if let Some(addr) = ts_addr
                                    && !token.is_empty()
                                {
                                    HostTransport::Tcp { addr, token }
                                } else {
                                    HostTransport::Ssh {
                                        target: target.clone(),
                                        remote_home: paired["home"].as_str().map(str::to_owned),
                                    }
                                };
                                finish_host_registration(
                                    ui,
                                    SavedHost {
                                        id: sanitize_host_id(&disp),
                                        name: disp,
                                        transport,
                                        ssh_target: Some(target),
                                        server_id: paired["server_id"].as_str().map(str::to_owned),
                                        enabled: true,
                                        added_at: canban_types::now(),
                                    },
                                    busy,
                                    feedback,
                                    feedback_tone,
                                );
                            }
                            Ok(Err(e)) => {
                                let msg = format!("{e:#}");
                                let _ = ssh_needs_bootstrap.try_set(true);
                                let _ = feedback_tone.try_set(Tone::Attention);
                                if msg.contains("canban_not_installed") {
                                    let _ = feedback.try_set(format!(
                                        "Canban is not yet installed on {target}. Click 'Install & Pair in Terminal…' to bootstrap `hc` + `canban` + systemd user service."
                                    ));
                                } else {
                                    let _ = feedback.try_set(format!(
                                        "{msg}. You can also run interactive setup via 'Install & Pair in Terminal…'."
                                    ));
                                }
                            }
                            Err(e) => {
                                let _ = feedback_tone.try_set(Tone::Danger);
                                let _ = feedback.try_set(e.to_string());
                            }
                        }
                    });
                })
                .disabled(move || busy.get()),
            ),
    )
}

fn connect_uri_tab(
    ui: Ui,
    busy: Signal<bool>,
    feedback: Signal<String>,
    feedback_tone: Signal<Tone>,
    uri_input: Signal<String>,
    uri_display_name: Signal<String>,
) -> View {
    view()
        .gap_3()
        .min_w(px(0.))
        .child(muted(
            "Run `canban pair --tailscale` on your remote Linux or macOS machine and paste the pairing URI below:",
        ))
        .child(
            view()
                .gap_1p5()
                .child(label("Pairing URI"))
                .child(field(uri_input, "canban://100.64.0.10:7680#<token>").label("Pairing URI")),
        )
        .child(
            view()
                .gap_1p5()
                .child(label("Display Name (optional)"))
                .child(field(uri_display_name, "Home Linux").label("Display name")),
        )
        .child(
            view()
                .flex_row()
                .justify_end()
                .pt_1()
                .child(
                    action("Connect Machine", Variant::Primary, move |_| {
                        let raw = uri_input.get_untracked();
                        let Some((addr, token)) = parse_pairing_uri(&raw) else {
                            feedback_tone.set(Tone::Danger);
                            feedback.set(
                                "Invalid pairing URI. Expected `canban://<host>:<port>#<token>`."
                                    .into(),
                            );
                            return;
                        };
                        let disp = {
                            let custom = uri_display_name.get_untracked();
                            if custom.trim().is_empty() {
                                addr.split(':').next().unwrap_or(&addr).to_owned()
                            } else {
                                custom.trim().to_owned()
                            }
                        };
                        finish_host_registration(
                            ui,
                            SavedHost {
                                id: sanitize_host_id(&disp),
                                name: disp,
                                transport: HostTransport::Tcp { addr, token },
                                ssh_target: None,
                                server_id: None,
                                enabled: true,
                                added_at: canban_types::now(),
                            },
                            busy,
                            feedback,
                            feedback_tone,
                        );
                    })
                    .disabled(move || busy.get()),
                ),
        )
}

/// `Settings -> Hosts` section: manage connected `canband` machines (Rename, Enable/Disable, Reconnect, Remove).
pub fn settings_section_view(ui: Ui) -> impl IntoElement {
    let renaming_id = signal(Option::<String>::None);
    let rename_draft = signal(String::new());
    let saved_map = signal(Vec::<SavedHost>::new());

    let reload_saved = move || {
        if let Some(reg) = crate::HOSTS.get() {
            saved_map.set(reg.saved_hosts());
            ui.hosts.set(reg.summaries());
        }
    };
    reload_saved();

    settings_section(
        "Hosts",
        "Connect Canban to remote Linux or macOS `canband` services over Tailscale or SSH while keeping This Mac active simultaneously.",
    )
    .trailing(
        view()
            .flex_row()
            .items_center()
            .gap_1p5()
            .child(compact_action(
                "+ Connect Machine…",
                Variant::Primary,
                move |_| open_connect_modal(ui),
            ))
            .child(compact_action(
                "Reconnect All",
                Variant::Quiet,
                move |_| reconnect_host(ui, None),
            )),
    )
    .child(dynamic(move || {
        let summaries = effective_hosts(ui);
        let saved = saved_map.get();
        let editing = renaming_id.get();

        let mut list = panel()
            .p_0()
            .gap_0()
            .border_1()
            .border_color(BORDER)
            .overflow_hidden();

        for (idx, h) in summaries.iter().enumerate() {
            let tone = status_tone(h);
            let is_enabled = h.is_local
                || saved
                    .iter()
                    .find(|s| s.id == h.id)
                    .map(|s| s.enabled)
                    .unwrap_or(true);
            let hid = h.id.clone();
            let hid_rec = h.id.clone();
            let hid_diag = h.id.clone();
            let hid_toggle = h.id.clone();
            let hid_remove = h.id.clone();
            let hid_edit = h.id.clone();
            let cur_name = h.name.clone();
            let is_editing = editing.as_deref() == Some(h.id.as_str());

            let mut controls = view().flex_row().flex_wrap().items_center().gap_1p5();
            if !h.is_local {
                if is_enabled {
                    if matches!(h.connection, HostConnectionState::ApiMismatch { .. }) {
                        let h_upd = h.clone();
                        controls = controls.child(compact_action(
                            "Update Daemon",
                            Variant::Primary,
                            move |_| update_host_daemon(ui, h_upd.clone()),
                        ));
                    }
                    controls = controls.child(compact_action(
                        "Reconnect",
                        Variant::Quiet,
                        move |_| reconnect_host(ui, Some(hid_rec.clone())),
                    ));
                }
                controls = controls
                    .child(compact_action(
                        if is_enabled { "Disable" } else { "Enable" },
                        Variant::Quiet,
                        move |_| {
                            if let Some(reg) = crate::HOSTS.get() {
                                if let Err(e) = reg.set_host_enabled(&hid_toggle, !is_enabled) {
                                    ui.operation_error.set(format!("{e:#}"));
                                } else {
                                    reload_saved();
                                    refresh(ui);
                                }
                            }
                        },
                    ))
                    .child(compact_action("Rename", Variant::Quiet, move |_| {
                        rename_draft.set(cur_name.clone());
                        renaming_id.set(Some(hid_edit.clone()));
                    }))
                    .child(compact_action("Remove", Variant::Danger, move |_| {
                        if let Some(reg) = crate::HOSTS.get() {
                            if let Err(e) = reg.remove_host(&hid_remove) {
                                ui.operation_error.set(format!("{e:#}"));
                            } else {
                                if ui.settings_host.get_untracked() == hid_remove {
                                    ui.settings_host.set("local".into());
                                }
                                reload_saved();
                                refresh(ui);
                                ui.notify("Removed remote host");
                            }
                        } else {
                            ui.hosts.update(|rows| rows.retain(|x| x.id != hid_remove));
                            ui.snapshot.update(|s| {
                                s.host_summaries.remove(&hid_remove);
                            });
                        }
                    }));
            }
            controls = controls.child(compact_action(
                "Diagnostics",
                Variant::Secondary,
                move |_| open_host_settings(ui, hid_diag.clone(), "diagnostics"),
            ));

            let mut row = view()
                .px_3p5()
                .py_3()
                .gap_1p5()
                .min_w(px(0.))
                .child(
                    view()
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
                                .min_w(px(0.))
                                .child(live_dot(dot_color_for_tone(tone)))
                                .child(label(h.name.clone()).font_weight(FontWeight::SEMIBOLD))
                                .child(badge(h.transport_label.clone()))
                                .child(status(h.connection.short_status(), tone)),
                        )
                        .child(controls),
                )
                .child(
                    muted(format!(
                        "Endpoint: {} · {} project{} · {}/{} active agents",
                        h.endpoint_label,
                        h.project_count,
                        if h.project_count == 1 { "" } else { "s" },
                        h.active_agents,
                        h.max_agents
                    ))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5)),
                );

            if is_editing {
                let save_id = hid.clone();
                row = row.child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .pt_1()
                        .child(
                            view()
                                .w(px(240.))
                                .child(field(rename_draft, "Host name").label("Host name")),
                        )
                        .child(compact_action("Save", Variant::Primary, move |_| {
                            let next = rename_draft.get_untracked();
                            if let Some(reg) = crate::HOSTS.get() {
                                match reg.rename_host(&save_id, &next) {
                                    Ok(()) => {
                                        renaming_id.set(None);
                                        reload_saved();
                                        refresh(ui);
                                    }
                                    Err(e) => ui.operation_error.set(format!("{e:#}")),
                                }
                            } else {
                                renaming_id.set(None);
                            }
                        }))
                        .child(compact_action("Cancel", Variant::Quiet, move |_| {
                            renaming_id.set(None);
                        })),
                );
            }

            if idx + 1 < summaries.len() {
                row = row.border_b_1().border_color(BORDER);
            }
            list = list.child(row);
        }
        list
    }))
    .child(
        panel()
            .p_3p5()
            .gap_1p5()
            .bg(INSET)
            .border_1()
            .border_color(BORDER)
            .child(
                label("Headless Linux / Remote CLI Quick Reference")
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(muted(
                "On any remote Linux or macOS machine, run `canban service install --tailscale && canban pair --tailscale` to enable autonomous 24/7 background execution and print a pairing URI.",
            )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{
        DiscoveryCatalog, HostConnectionState, HostSummary, Project, Snapshot, SshHostEntry,
        TailscalePeer, Workflow,
    };
    use nsh_core::{ApproxTextEngine, Core, MountItem, geometry::size};
    use std::collections::BTreeMap;

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

    #[test]
    fn trigger_pill_and_offline_banner_reflect_multi_host_state() {
        #[component]
        fn ProbeOfflineHost() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut snapshot = Snapshot::default();
            snapshot.projects.push(Project {
                id: "home-linux:proj-remote".into(),
                name: "Backend Worker".into(),
                directory: "/home/jjy/backend".into(),
                location: None,
                github: None,
                detected_github: None,
                github_disconnected: false,
                delivery_mode: Default::default(),
                agent_overrides: BTreeMap::new(),
                tags: Vec::new(),
                workflow: Workflow::default(),
                target_branch: Some("main".into()),
                created_at: 1,
            });
            let local_summary = HostSummary {
                id: "local".into(),
                name: "This Mac".into(),
                is_local: true,
                transport_label: "Local socket".into(),
                endpoint_label: "~/.canban/app.sock".into(),
                ssh_target: None,
                server_id: None,
                connection: HostConnectionState::Connected {
                    rtt_ms: 0,
                    api_version: canban_updater::DESKTOP_API,
                    version: "0.4.2".into(),
                    platform: "macos-aarch64".into(),
                },
                active_agents: 0,
                max_agents: 5,
                project_count: 0,
                reconnect_count: 0,
                last_synced_at: None,
            };
            let remote_summary = HostSummary {
                id: "home-linux".into(),
                name: "Home Linux".into(),
                is_local: false,
                transport_label: "Tailscale TCP".into(),
                endpoint_label: "100.64.0.10:7680".into(),
                ssh_target: Some("jjy@home-box".into()),
                server_id: Some("srv-home".into()),
                connection: HostConnectionState::Offline {
                    error: "connection refused".into(),
                    cached_at: Some(1_700_000_000_000),
                },
                active_agents: 0,
                max_agents: 8,
                project_count: 1,
                reconnect_count: 2,
                last_synced_at: Some(1_700_000_000_000),
            };
            snapshot
                .entity_hosts
                .insert("home-linux:proj-remote".into(), "home-linux".into());
            snapshot
                .host_summaries
                .insert("local".into(), local_summary.clone());
            snapshot
                .host_summaries
                .insert("home-linux".into(), remote_summary.clone());
            ui.hosts.set(vec![local_summary, remote_summary]);
            ui.snapshot.set(snapshot);
            ui.project.set("home-linux:proj-remote".into());

            view()
                .w(px(1200.))
                .h(px(200.))
                .child(trigger_pill(ui))
                .child(offline_banner(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(200.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbeOfflineHost>());
        let texts = collect_mounted_texts(core.commit().expect("mount batch"));

        assert!(
            texts.iter().any(|t| t == "1/2"),
            "expected trigger pill online ratio `1/2` (This Mac online + 1 remote offline), got {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Home Linux is offline") && t.contains("read-only")),
            "expected offline read-only banner for Home Linux, got {texts:?}"
        );
    }

    #[test]
    fn connect_modal_lists_tailscale_peers_and_parse_pairing_uri_roundtrips() {
        assert_eq!(
            parse_pairing_uri("canban://100.64.0.10:7680#secret-token-12345678"),
            Some(("100.64.0.10:7680".into(), "secret-token-12345678".into()))
        );
        assert_eq!(parse_pairing_uri("https://example.com"), None);

        #[component]
        fn ProbeConnectModal() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.discovery.set(DiscoveryCatalog {
                tailscale_available: true,
                tailscale_self_ip: Some("100.64.0.1".into()),
                tailscale_self_host: Some("macbook".into()),
                tailscale_error: None,
                tailscale_peers: vec![TailscalePeer {
                    hostname: "home-box".into(),
                    dns_name: "home-box.tailnet.ts.net".into(),
                    ips: vec!["100.64.0.10".into()],
                    os: "linux".into(),
                    online: true,
                }],
                ssh_hosts: vec![SshHostEntry {
                    alias: "home-box".into(),
                    hostname: "100.64.0.10".into(),
                    user: Some("jjy".into()),
                    port: None,
                }],
            });
            ui.utility_popup.set("connect-host".into());
            view().w(px(1200.)).h(px(800.)).child(connect_modal(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(800.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbeConnectModal>());
        let texts = collect_mounted_texts(core.commit().expect("mount batch"));

        assert!(
            texts.iter().any(|t| t == "Connect a Machine"),
            "expected modal title `Connect a Machine`, got {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "home-box"),
            "expected discovered Tailscale peer `home-box`, got {texts:?}"
        );
    }

    #[test]
    fn api_mismatch_surfaces_update_daemon_action_in_popover_and_settings() {
        #[component]
        fn ProbeMismatchHosts() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let local_summary = HostSummary {
                id: "local".into(),
                name: "This Mac".into(),
                is_local: true,
                transport_label: "Local socket".into(),
                endpoint_label: "~/.canban/app.sock".into(),
                ssh_target: None,
                server_id: None,
                connection: HostConnectionState::Connected {
                    rtt_ms: 0,
                    api_version: canban_updater::DESKTOP_API,
                    version: "0.4.2".into(),
                    platform: "macos-aarch64".into(),
                },
                active_agents: 0,
                max_agents: 5,
                project_count: 0,
                reconnect_count: 0,
                last_synced_at: None,
            };
            let mismatch_summary = HostSummary {
                id: "home-linux".into(),
                name: "Home Linux".into(),
                is_local: false,
                transport_label: "Tailscale / TCP".into(),
                endpoint_label: "100.64.0.10:7680".into(),
                ssh_target: Some("jjy@100.64.0.10".into()),
                server_id: Some("srv-home".into()),
                connection: HostConnectionState::ApiMismatch {
                    server_api: 54,
                    app_api: canban_updater::DESKTOP_API,
                    version: "20261115-2".into(),
                },
                active_agents: 0,
                max_agents: 8,
                project_count: 1,
                reconnect_count: 0,
                last_synced_at: None,
            };
            ui.hosts.set(vec![local_summary, mismatch_summary]);
            ui.utility_popup.set("hosts".into());
            view()
                .w(px(1200.))
                .h(px(900.))
                .child(settings_section_view(ui))
                .child(popover(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(900.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbeMismatchHosts>());
        let texts = collect_mounted_texts(core.commit().expect("mount batch"));

        let update_buttons = texts.iter().filter(|t| *t == "Update Daemon").count();
        assert!(
            update_buttons >= 2,
            "expected `Update Daemon` action in both Settings -> Hosts and Connected Hosts popover, got {texts:?}"
        );
    }
}
