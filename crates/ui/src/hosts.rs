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

pub(crate) fn sync_chip_spec(mark: SyncMark) -> (&'static str, Tone) {
    match mark {
        SyncMark::Pending => ("Not synced", Tone::Neutral),
        SyncMark::Conflict | SyncMark::Failed => ("Sync issue", Tone::Attention),
    }
}

fn format_sync_counts(pending: usize, conflicts: usize) -> String {
    match (pending, conflicts) {
        (0, 0) => String::new(),
        (p, 0) => format!("{p} pending"),
        (0, c) => format!("{c} sync issue{}", if c == 1 { "" } else { "s" }),
        (p, c) => format!(
            "{p} pending · {c} sync issue{}",
            if c == 1 { "" } else { "s" }
        ),
    }
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
    let active = ui.snapshot.with(crate::backend::active_agent_count);
    let projects = ui.snapshot.with(|s| s.projects.len());
    vec![HostSummary {
        id: "local".into(),
        name: crate::platform::LOCAL_HOST_NAME.into(),
        is_local: true,
        transport_label: "Local socket".into(),
        endpoint_label: "~/.canban/app.sock".into(),
        ssh_target: None,
        server_id: None,
        connection: HostConnectionState::Connected {
            rtt_ms: 0,
            api_version: crate::backend::desktop_api(),
            version: env!("CARGO_PKG_VERSION").into(),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        },
        active_agents: active,
        max_agents: ui.max_agents.get(),
        project_count: projects,
        reconnect_count: 0,
        last_synced_at: None,
        pending_mutations: 0,
        sync_conflicts: 0,
    }]
}

/// Human-readable display name for a `canband` host (`"This Mac"` for `"local"`).
pub fn host_name(ui: Ui, host_id: &str) -> String {
    if host_id.is_empty() || host_id == "local" {
        return crate::platform::LOCAL_HOST_NAME.into();
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

pub fn open_sync_modal(ui: Ui) {
    crate::utilities::open(ui, "sync");
}

pub fn close_overlays(ui: Ui) {
    let cur = ui.utility_popup.get_untracked();
    if cur == "hosts" || cur == "connect-host" || cur == "sync" {
        crate::utilities::close(ui);
    }
}

pub fn refresh_discovery(ui: Ui) {
    #[cfg(feature = "local-host")]
    {
        if ui.discovery_loading.get_untracked() || !crate::backend::is_connected() {
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
    #[cfg(not(feature = "local-host"))]
    {
        let _ = ui;
    }
}

pub fn reconnect_host(ui: Ui, host_id: Option<String>) {
    if let Some(backend) = crate::backend::get().filter(|b| b.has_host_registry()) {
        ui.spawn(async move {
            let target = host_id.clone();
            let _ = blocking(move || {
                backend.reconnect_host(target.as_deref());
                backend.poll_remotes_once();
                (backend.host_summaries(), backend.merged_snapshot())
            })
            .await
            .map(|(sums, snap_res)| {
                let _ = ui.hosts.try_set(sums);
                if let Ok(snap) = snap_res {
                    ui.accept_snapshot(snap);
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

fn mark_host_connection(ui: Ui, host_id: &str, next: HostConnectionState) {
    let cur_list = untracked(|| effective_hosts(ui));
    ui.hosts.set(
        cur_list
            .into_iter()
            .map(|mut h| {
                if h.id == host_id {
                    h.connection = next.clone();
                }
                h
            })
            .collect(),
    );
    ui.snapshot.update(|s| {
        if let Some(h) = s.host_summaries.get_mut(host_id) {
            h.connection = next.clone();
        }
    });
}

pub fn update_host_daemon(ui: Ui, summary: HostSummary) {
    if summary.is_local || summary.is_updating() {
        return;
    }
    let already_updating = untracked(|| {
        effective_hosts(ui)
            .iter()
            .any(|h| h.id == summary.id && h.is_updating())
    });
    if already_updating {
        return;
    }
    let host_id = summary.id.clone();
    let host_name = summary.name.clone();
    let ssh_fallback = infer_ssh_target_for_host(&summary);
    let prefer_tailscale = !summary.transport_label.contains("SSH");

    mark_host_connection(
        ui,
        &host_id,
        HostConnectionState::Updating {
            version: String::new(),
        },
    );

    if let Some(backend) = crate::backend::get().filter(|b| b.has_host_registry()) {
        ui.notify(format!("Updating Canban daemon on {host_name}…"));
        ui.spawn(async move {
            let hid = host_id.clone();
            let res = blocking(move || {
                let rpc_res = backend.update_remote_daemon(&hid);
                let sums = backend.host_summaries();
                let snap = backend.merged_snapshot();
                (rpc_res, sums, snap)
            })
            .await;
            match res {
                Ok((Ok(val), sums, snap_res)) => {
                    let _ = ui.hosts.try_set(sums);
                    if let Ok(snap) = snap_res {
                        ui.accept_snapshot(snap);
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
                        ui.accept_snapshot(snap);
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
        ui.notify(format!("Gallery preview: updating daemon on {host_name}…"));
        let hid = host_id;
        ui.owner.run(|| {
            set_timeout(Duration::from_millis(1600), move || {
                let target_ver = env!("CARGO_PKG_VERSION").to_owned();
                let connected = HostConnectionState::Connected {
                    rtt_ms: 9,
                    api_version: crate::backend::desktop_api(),
                    version: target_ver.clone(),
                    platform: "linux-x86_64".into(),
                };
                if let Some(mut rows) = ui.hosts.try_get() {
                    let mut changed = false;
                    for h in &mut rows {
                        if h.id == hid && h.is_updating() {
                            h.connection = connected.clone();
                            changed = true;
                        }
                    }
                    if changed {
                        let _ = ui.hosts.try_set(rows);
                    }
                }
                if let Some(mut snap) = ui.snapshot.try_get()
                    && let Some(h) = snap.host_summaries.get_mut(&hid)
                    && h.is_updating()
                {
                    h.connection = connected.clone();
                    ui.accept_snapshot(snap);
                }
                ui.notify(format!("Updated {host_name} daemon to {target_ver}"));
            });
        });
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
    #[cfg(feature = "local-host")]
    {
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
    #[cfg(not(feature = "local-host"))]
    {
        let _ = (ssh_target, prefer_tailscale, port);
        ui.operation_error
            .set("SSH bootstrap requires the local-host feature".into());
    }
}

pub(crate) fn status_tone(summary: &HostSummary) -> Tone {
    if summary.is_connected() {
        Tone::Success
    } else if summary.is_updating() {
        Tone::Info
    } else if summary.needs_attention() {
        Tone::Danger
    } else {
        Tone::Attention
    }
}

pub(crate) fn host_status_badge(summary: &HostSummary) -> View {
    let tone = status_tone(summary);
    let label = summary.connection.short_status();
    if summary.is_updating() {
        live_status(label, tone)
    } else {
        status(label, tone)
    }
}

pub(crate) fn dot_color_for_tone(tone: Tone) -> ThemeColor {
    match tone {
        Tone::Success => SUCCESS,
        Tone::Danger => DANGER,
        Tone::Attention => ATTENTION,
        Tone::Info => ACCENT,
        Tone::Highlight => HIGHLIGHT,
        Tone::Neutral => MUTED,
    }
}

/// Top-chrome status pill mounted next to `Activity & Notifications`.
pub fn trigger_pill(ui: Ui) -> impl IntoElement {
    let hosts_memo = computed(move || effective_hosts(ui));
    let pill_summary = computed(move || {
        hosts_memo.with(|list| {
            let remote_count = list.iter().filter(|h| !h.is_local).count();
            let any_attention = list.iter().any(|h| !h.is_local && h.needs_attention());
            let updating_count = list.iter().filter(|h| h.is_updating()).count();
            let any_updating = list.iter().any(|h| !h.is_local && h.is_updating());
            let any_offline = list
                .iter()
                .any(|h| !h.is_local && h.is_offline() && !h.is_updating());
            let online_remotes = list
                .iter()
                .filter(|h| !h.is_local && h.is_connected())
                .count();
            let pending_total: usize = list
                .iter()
                .filter(|h| !h.is_local)
                .map(|h| h.pending_mutations)
                .sum();
            let conflict_total: usize = list
                .iter()
                .filter(|h| !h.is_local)
                .map(|h| h.sync_conflicts)
                .sum();
            (
                remote_count,
                any_attention,
                any_updating,
                any_offline,
                online_remotes,
                updating_count,
                pending_total,
                conflict_total,
            )
        })
    });
    let is_open = computed(move || {
        let p = ui.utility_popup.get();
        p == "hosts" || p == "connect-host" || p == "sync"
    });

    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(
            hint_tooltip(move || {
                let (remote_count, _, _, _, online, updating, pending_total, conflict_total) =
                    pill_summary.get();
                let sync_tip = format_sync_counts(pending_total, conflict_total);
                let sync_suffix = if sync_tip.is_empty() {
                    String::new()
                } else {
                    format!("\n{sync_tip}")
                };
                if remote_count == 0 {
                    format!(
                        "Connected Hosts\n{} (local) · Click to connect a remote machine",
                        crate::platform::LOCAL_HOST_NAME
                    )
                } else if updating > 0 {
                    format!(
                        "Connected Hosts\n{} + {} / {} remote online ({} updating…){sync_suffix}",
                        crate::platform::LOCAL_HOST_NAME,
                        online,
                        remote_count,
                        updating
                    )
                } else {
                    format!(
                        "Connected Hosts\n{} + {} / {} remote online{sync_suffix}",
                        crate::platform::LOCAL_HOST_NAME,
                        online,
                        remote_count
                    )
                }
            })
            .placement(Placement::Bottom),
        )
        .child(
            press_target()
                .h(px(28.))
                .px_2p5()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(move || {
                    let (_, any_attention, any_updating, any_offline, _, _, _, conflict_total) =
                        pill_summary.get();
                    if is_open.get() {
                        ACCENT
                    } else if any_attention {
                        DANGER
                    } else if any_updating {
                        ACCENT
                    } else if any_offline || conflict_total > 0 {
                        ATTENTION
                    } else {
                        BORDER
                    }
                })
                .bg(move || {
                    let (_, any_attention, any_updating, any_offline, _, _, _, conflict_total) =
                        pill_summary.get();
                    if is_open.get() {
                        SELECTED
                    } else if any_attention || any_offline || conflict_total > 0 {
                        ATTENTION_SURFACE
                    } else if any_updating {
                        SELECTED
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
                    let (
                        remote_count,
                        _,
                        any_updating,
                        _,
                        online_remotes,
                        _,
                        pending_total,
                        conflict_total,
                    ) = pill_summary.get();
                    let sync_note = format_sync_counts(pending_total, conflict_total);
                    let suffix = if sync_note.is_empty() {
                        String::new()
                    } else {
                        format!(", {sync_note}")
                    };
                    Some(
                        if remote_count == 0 {
                            format!("Connected Hosts, {} only", crate::platform::LOCAL_HOST_NAME)
                        } else if any_updating {
                            format!(
                                "Connected Hosts, updating remote daemon ({} of {} online){suffix}",
                                online_remotes, remote_count
                            )
                        } else {
                            format!(
                                "Connected Hosts, {} of {} remote online{suffix}",
                                online_remotes, remote_count
                            )
                        }
                        .into(),
                    )
                })
                .on_press(move |_: &()| toggle_popover(ui))
                .child(
                    icon_tinted(Icon::Server, move || {
                        let (_, any_attention, any_updating, any_offline, _, _, _, conflict_total) =
                            pill_summary.get();
                        if is_open.get() {
                            ACCENT
                        } else if any_attention {
                            DANGER
                        } else if any_updating {
                            ACCENT
                        } else if any_offline || conflict_total > 0 {
                            ATTENTION
                        } else {
                            MUTED
                        }
                    })
                    .w(px(13.5))
                    .h(px(13.5)),
                )
                .child(when(
                    move || pill_summary.get().0 > 0,
                    move || {
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .child(when(
                                move || pill_summary.get().2,
                                move || {
                                    live_dot({
                                        let (
                                            _,
                                            any_attention,
                                            any_updating,
                                            any_offline,
                                            _,
                                            _,
                                            _,
                                            conflict_total,
                                        ) = pill_summary.get_untracked();
                                        if any_attention {
                                            DANGER
                                        } else if any_updating {
                                            ACCENT
                                        } else if any_offline || conflict_total > 0 {
                                            ATTENTION
                                        } else {
                                            SUCCESS
                                        }
                                    })
                                },
                            ))
                            .child(when(
                                move || !pill_summary.get().2,
                                move || {
                                    view().w(px(6.)).h(px(6.)).rounded_full().bg(move || {
                                        let (
                                            remote_count,
                                            any_attention,
                                            any_updating,
                                            any_offline,
                                            _,
                                            _,
                                            _,
                                            conflict_total,
                                        ) = pill_summary.get();
                                        if any_attention {
                                            DANGER
                                        } else if any_updating {
                                            ACCENT
                                        } else if any_offline || conflict_total > 0 {
                                            ATTENTION
                                        } else if remote_count > 0 {
                                            SUCCESS
                                        } else {
                                            MUTED
                                        }
                                    })
                                },
                            ))
                            .child(
                                muted(move || {
                                    let (remote_count, _, _, _, online_remotes, _, _, _) =
                                        pill_summary.get();
                                    format!("{}/{}", online_remotes + 1, remote_count + 1)
                                })
                                .text_size(px(11.5))
                                .font_family(MONO_FONT),
                            )
                            .child(when(
                                move || pill_summary.get().7 > 0,
                                move || {
                                    status(
                                        move || {
                                            SharedString::from(format!("{}!", pill_summary.get().7))
                                        },
                                        Tone::Attention,
                                    )
                                },
                            ))
                            .child(when(
                                move || pill_summary.get().6 > 0 && pill_summary.get().7 == 0,
                                move || {
                                    muted(move || format!("· {}", pill_summary.get().6))
                                        .text_size(px(11.))
                                        .font_family(MONO_FONT)
                                },
                            ))
                    },
                )),
        )
}

#[derive(Clone)]
struct PopoverHostRow {
    summary: HostSummary,
    err_opt: Option<String>,
    status_key: String,
    initial_status: String,
    is_conn: bool,
    is_upd: bool,
    is_mismatch: bool,
    needs_attn: bool,
}

impl PopoverHostRow {
    fn from_summary(summary: HostSummary) -> Self {
        let err_opt = summary.connection.error_detail().map(str::to_owned);
        let initial_status = summary.connection.short_status();
        let is_conn = summary.is_connected();
        let status_key = if is_conn {
            "Connected".to_owned()
        } else {
            initial_status.clone()
        };
        let is_upd = summary.is_updating();
        let is_mismatch = matches!(summary.connection, HostConnectionState::ApiMismatch { .. });
        let needs_attn = summary.needs_attention();
        Self {
            summary,
            err_opt,
            status_key,
            initial_status,
            is_conn,
            is_upd,
            is_mismatch,
            needs_attn,
        }
    }
}

impl PartialEq for PopoverHostRow {
    fn eq(&self, other: &Self) -> bool {
        self.summary.id == other.summary.id
            && self.summary.name == other.summary.name
            && self.summary.is_local == other.summary.is_local
            && self.summary.transport_label == other.summary.transport_label
            && self.summary.endpoint_label == other.summary.endpoint_label
            && self.summary.project_count == other.summary.project_count
            && self.summary.active_agents == other.summary.active_agents
            && self.summary.max_agents == other.summary.max_agents
            && self.summary.pending_mutations == other.summary.pending_mutations
            && self.summary.sync_conflicts == other.summary.sync_conflicts
            && self.err_opt == other.err_opt
            && self.status_key == other.status_key
            && self.is_conn == other.is_conn
            && self.is_upd == other.is_upd
            && self.is_mismatch == other.is_mismatch
            && self.needs_attn == other.needs_attn
    }
}

/// Top-right anchored `Hosts` quick popover (`popup == "hosts"`).
pub fn popover(ui: Ui) -> impl IntoElement {
    let rows_memo = computed(move || {
        effective_hosts(ui)
            .into_iter()
            .map(PopoverHostRow::from_summary)
            .collect::<Vec<_>>()
    });
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
                            let list = rows_memo.get();
                            let mut col = view().gap_0().min_w(px(0.));
                            for (idx, row_entry) in list.iter().enumerate() {
                                let h = &row_entry.summary;
                                let hid_term = h.id.clone();
                                let hid_rec = h.id.clone();
                                let hid_set = h.id.clone();
                                let hid_diag = h.id.clone();
                                let tone = status_tone(h);
                                let err_opt = row_entry.err_opt.clone();
                                let is_local = h.is_local;
                                let is_conn = row_entry.is_conn;
                                let is_upd = row_entry.is_upd;
                                let hid_status = h.id.clone();
                                let initial_status = row_entry.initial_status.clone();
                                let status_text = move || {
                                    effective_hosts(ui)
                                        .into_iter()
                                        .find(|row| row.id == hid_status)
                                        .map(|row| {
                                            SharedString::from(row.connection.short_status())
                                        })
                                        .unwrap_or_else(|| {
                                            SharedString::from(initial_status.clone())
                                        })
                                };
                                let status_chip = if is_upd {
                                    live_status(status_text, tone)
                                } else {
                                    status(status_text, tone)
                                };

                                let mut actions =
                                    view().flex_row().flex_wrap().items_center().gap_1();
                                if !is_local {
                                    if is_upd {
                                        actions = actions.child(
                                            compact_action("Updating…", Variant::Secondary, |_| {})
                                                .disabled(|| true),
                                        );
                                    } else {
                                        if row_entry.is_mismatch {
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
                                            .child(status_chip),
                                    )
                                    .child({
                                        let mut meta = view()
                                            .flex_row()
                                            .flex_wrap()
                                            .items_center()
                                            .gap_1p5()
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
                                            );
                                        if h.pending_mutations > 0 {
                                            meta = meta.child(badge(format!(
                                                "{} pending",
                                                h.pending_mutations
                                            )));
                                        }
                                        if h.sync_conflicts > 0 {
                                            meta = meta.child(status(
                                                format!(
                                                    "{} sync issue{}",
                                                    h.sync_conflicts,
                                                    if h.sync_conflicts == 1 { "" } else { "s" }
                                                ),
                                                Tone::Attention,
                                            ));
                                        }
                                        view()
                                            .flex_row()
                                            .flex_wrap()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(meta)
                                            .child(actions)
                                    });

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
                                        .child(when(
                                            move || {
                                                rows_memo.with(|list| {
                                                    list.iter().any(|r| {
                                                        r.summary.pending_mutations > 0
                                                            || r.summary.sync_conflicts > 0
                                                    })
                                                })
                                            },
                                            move || {
                                                compact_action(
                                                    "Sync…",
                                                    Variant::Quiet,
                                                    move |_| open_sync_modal(ui),
                                                )
                                            },
                                        ))
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
                .child(muted(format!(
                    "Creating Project on {}.",
                    crate::platform::LOCAL_HOST_NAME
                )))
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
                let is_updating = h.is_updating();
                let title = if is_online {
                    h.name.clone()
                } else if is_updating {
                    format!("{} (updating…)", h.name)
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
                                if is_updating {
                                    "Wait for the remote daemon update to finish before creating a new Project on this host."
                                } else {
                                    "Reconnect this remote host before creating a new Project on it."
                                }
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
                let list = effective_hosts(ui);
                let host_sum = list
                    .into_iter()
                    .find(|h| h.id == host_id)
                    .or_else(|| ui.snapshot.with(|s| s.host_summary(&host_id).cloned()));
                let host_name = host_sum
                    .as_ref()
                    .map(|h| h.name.clone())
                    .unwrap_or_else(|| {
                        ui.snapshot
                            .with(|s| s.host_display_name(&host_id).to_owned())
                    });
                let is_updating = host_sum.as_ref().is_some_and(HostSummary::is_updating);
                let pending_mutations = host_sum.as_ref().map(|h| h.pending_mutations).unwrap_or(0);
                let sync_conflicts = host_sum.as_ref().map(|h| h.sync_conflicts).unwrap_or(0);
                let detail = host_sum
                    .as_ref()
                    .map(|h| h.connection.short_status())
                    .unwrap_or_else(|| "Offline · Cached".into());
                let mismatch_sum = host_sum
                    .filter(|h| matches!(h.connection, HostConnectionState::ApiMismatch { .. }));
                let hid_rec = host_id.clone();
                let hid_diag = host_id;
                let mut trailing = view().flex_row().items_center().gap_1p5();
                if pending_mutations > 0 || sync_conflicts > 0 {
                    trailing =
                        trailing.child(compact_action("Sync…", Variant::Secondary, move |_| {
                            open_sync_modal(ui)
                        }));
                }
                if is_updating {
                    trailing = trailing.child(
                        compact_action("Updating…", Variant::Secondary, |_| {}).disabled(|| true),
                    );
                } else {
                    if let Some(h_upd) = mismatch_sum {
                        trailing = trailing.child(compact_action(
                            "Update Daemon",
                            Variant::Primary,
                            move |_| update_host_daemon(ui, h_upd.clone()),
                        ));
                    }
                    trailing = trailing.child(compact_action(
                        "Reconnect",
                        Variant::Secondary,
                        move |_| {
                            reconnect_host(ui, Some(hid_rec.clone()));
                        },
                    ));
                }
                trailing =
                    trailing.child(compact_action("Diagnostics", Variant::Quiet, move |_| {
                        open_host_settings(ui, hid_diag.clone(), "diagnostics");
                    }));
                let banner_bg = if is_updating {
                    SELECTED
                } else {
                    ATTENTION_SURFACE
                };
                let banner_border = if is_updating { ACCENT } else { ATTENTION };
                let sync_counts = format_sync_counts(pending_mutations, sync_conflicts);
                let suffix = if sync_counts.is_empty() {
                    String::new()
                } else {
                    format!(" · {sync_counts}")
                };
                let banner_text = if is_updating {
                    format!(
                        "{host_name} is updating ({detail}) · showing cached data · edits sync when it reconnects{suffix}"
                    )
                } else {
                    format!(
                        "{host_name} is offline ({detail}) · showing cached data · edits sync when it reconnects{suffix}"
                    )
                };
                view()
                    .px_4()
                    .py_2()
                    .bg(banner_bg)
                    .border_b_1()
                    .border_color(banner_border)
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
                            .child(live_dot(banner_border))
                            .child(label(banner_text).text_size(px(12.5))),
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
    let Some(backend) = crate::backend::get().filter(|b| b.has_host_registry()) else {
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
                api_version: crate::backend::desktop_api(),
                version: env!("CARGO_PKG_VERSION").into(),
                platform: "linux-x86_64".into(),
            },
            active_agents: 0,
            max_agents: 8,
            project_count: 0,
            reconnect_count: 0,
            last_synced_at: Some(canban_types::now()),
            pending_mutations: 0,
            sync_conflicts: 0,
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
    };
    busy.set(true);
    feedback.set(format!("Connecting to {}…", saved.name));
    feedback_tone.set(Tone::Neutral);
    ui.spawn(async move {
        let host_name = saved.name.clone();
        let res = blocking(move || {
            let sum = backend.add_host(saved)?;
            let snap = backend.merged_snapshot()?;
            let all = backend.host_summaries();
            Ok::<_, anyhow::Error>((sum, snap, all))
        })
        .await;
        let _ = busy.try_set(false);
        match res {
            Ok(Ok((_sum, snap, all))) => {
                let _ = ui.hosts.try_set(all);
                ui.accept_snapshot(snap);
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
                format!(
                    "Tailscale CLI is not running on {}. Start Tailscale or connect via SSH / Pairing URI.",
                    crate::platform::LOCAL_HOST_NAME
                )
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
                            if !crate::backend::is_connected() {
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
                                    #[cfg(feature = "local-host")]
                                    {
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
                                    }
                                    #[cfg(not(feature = "local-host"))]
                                    {
                                        let _ = (ip_c, ssh_c);
                                        (
                                            Err::<serde_json::Value, anyhow::Error>(anyhow::anyhow!(
                                                "Host probing requires the local-host feature"
                                            )),
                                            Err::<serde_json::Value, anyhow::Error>(anyhow::anyhow!(
                                                "SSH pairing requires the local-host feature"
                                            )),
                                        )
                                    }
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
                    if !crate::backend::is_connected() {
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
                            #[cfg(feature = "local-host")]
                            {
                                canban_app_server::network::pair_via_ssh_noninteractive(
                                    &t_clone,
                                    prefer_ts,
                                    DEFAULT_DAEMON_PORT,
                                )
                            }
                            #[cfg(not(feature = "local-host"))]
                            {
                                let _ = (t_clone, prefer_ts);
                                Err::<serde_json::Value, anyhow::Error>(anyhow::anyhow!(
                                    "SSH pairing requires the local-host feature"
                                ))
                            }
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

/// Sync queue & conflict resolution modal (`popup == "sync"`).
pub fn sync_modal(ui: Ui) -> impl IntoElement {
    let refresh = signal(0u64);
    let items = signal(if crate::backend::is_connected() {
        crate::backend::sync_items()
    } else {
        ui.sync_preview.get_untracked()
    });
    let busy_id = signal(None::<String>);
    let feedback = signal(None::<(String, Tone)>);
    let editing_id = signal(None::<String>);
    let edit_title = signal(String::new());
    let edit_body = signal(String::new());

    effect(move || {
        let _ = refresh.get();
        let _ = ui.hosts.get();
        let _ = ui.snapshot_epoch.get();
        if crate::backend::is_connected() {
            ui.spawn(async move {
                if let Ok(loaded) = blocking(crate::backend::sync_items).await {
                    let _ = items.try_set(loaded);
                }
            });
        } else {
            items.set(ui.sync_preview.get());
        }
    });

    let dispatch_sync_action = move |host_id: String,
                                     mutation_id: String,
                                     sync_action: SyncAction| {
        if busy_id.get_untracked().is_some() {
            return;
        }
        feedback.set(None);
        if !crate::backend::is_connected() {
            ui.sync_preview.update(|list| {
                list.retain(|entry| {
                    !(entry.host_id == host_id && entry.mutation_id == mutation_id)
                });
            });
            if editing_id.get_untracked().as_deref() == Some(mutation_id.as_str()) {
                editing_id.set(None);
            }
            return;
        }
        busy_id.set(Some(mutation_id.clone()));
        ui.spawn(async move {
            let res =
                blocking(move || crate::backend::sync_action(&host_id, &mutation_id, sync_action))
                    .await;
            let _ = busy_id.try_set(None);
            match res {
                Ok(Ok(())) => {
                    let _ = editing_id.try_set(None);
                    refresh.update(|v| *v += 1);
                    crate::presenter::refresh(ui);
                }
                Ok(Err(e)) => {
                    let msg = format!("Sync action failed: {e}");
                    let _ = feedback.try_set(Some((msg.clone(), Tone::Danger)));
                    ui.notify(msg);
                }
                Err(e) => {
                    let msg = format!("Sync action failed: {e}");
                    let _ = feedback.try_set(Some((msg.clone(), Tone::Danger)));
                    ui.notify(msg);
                }
            }
        });
    };

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
                        .label("Close Sync")
                        .on_press(move || close_overlays(ui)),
                )
                .child(
                    panel()
                        .w(px(560.))
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
                                            label("Sync")
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_size(px(15.)),
                                        )
                                        .child(muted(
                                            "Pending offline changes and conflicts waiting to sync cleanly.",
                                        )),
                                )
                                .child(chrome_button(Icon::Close, "Close", move |_| {
                                    close_overlays(ui)
                                })),
                        )
                        .child(
                            scroll()
                                .p_4()
                                .max_h(px(480.))
                                .child(dynamic(move || {
                                    let list = items.get();
                                    let fb = feedback.get();
                                    let mut body = view().gap_2p5().min_w(px(0.));
                                    if let Some((msg, tone)) = fb {
                                        body = body.child(notice(msg, tone));
                                    }
                                    if list.is_empty() {
                                        return body.child(muted("Everything is synced"));
                                    }
                                    for item in list {
                                        let state_chip = match item.state {
                                            SyncItemState::Queued => {
                                                status("Pending", Tone::Neutral).into_element()
                                            }
                                            SyncItemState::InFlight => {
                                                live_status("Syncing", Tone::Info).into_element()
                                            }
                                            SyncItemState::Conflict => {
                                                status("Conflict", Tone::Attention).into_element()
                                            }
                                            SyncItemState::Failed => {
                                                status("Failed", Tone::Danger).into_element()
                                            }
                                        };
                                        let title_text = if item.card_title.is_empty() {
                                            item.card_id.clone()
                                        } else {
                                            item.card_title.clone()
                                        };
                                        let mut row = view()
                                            .p_3()
                                            .rounded(px(RADIUS))
                                            .border_1()
                                            .border_color(BORDER)
                                            .bg(INSET)
                                            .gap_2()
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
                                                            .flex_1()
                                                            .min_w(px(0.))
                                                            .flex_wrap()
                                                            .items_center()
                                                            .gap_1p5()
                                                            .child(
                                                                label(title_text)
                                                                    .font_weight(
                                                                        FontWeight::SEMIBOLD,
                                                                    )
                                                                    .truncate(),
                                                            )
                                                            .child(badge(item.host_name.clone()))
                                                            .child(badge(item.kind.clone())),
                                                    )
                                                    .child(state_chip),
                                            );
                                        if !item.message.is_empty() {
                                            row = row.child(
                                                muted(item.message.clone())
                                                    .text_size(px(12.))
                                                    .text_color(
                                                        if item.state == SyncItemState::Failed {
                                                            DANGER
                                                        } else {
                                                            MUTED
                                                        },
                                                    ),
                                            );
                                        }
                                        if item.state == SyncItemState::Conflict
                                            && !item.conflict_fields.is_empty()
                                        {
                                            let mut diffs = view()
                                                .p_2p5()
                                                .rounded(px(RADIUS))
                                                .bg(SURFACE)
                                                .border_1()
                                                .border_color(BORDER)
                                                .gap_1p5()
                                                .min_w(px(0.));
                                            for field_name in &item.conflict_fields {
                                                let mine_val = item
                                                    .mine
                                                    .get(field_name)
                                                    .cloned()
                                                    .unwrap_or_default();
                                                let srv_val = item
                                                    .server
                                                    .get(field_name)
                                                    .cloned()
                                                    .unwrap_or_default();
                                                diffs = diffs
                                                    .child(
                                                        muted(format!(
                                                            "{field_name} · Mine: {mine_val}"
                                                        ))
                                                        .text_size(px(11.5))
                                                        .truncate(),
                                                    )
                                                    .child(
                                                        muted(format!(
                                                            "{field_name} · Server: {srv_val}"
                                                        ))
                                                        .text_size(px(11.5))
                                                        .truncate(),
                                                    );
                                            }
                                            row = row.child(diffs);
                                        }

                                        let mut actions = view()
                                            .flex_row()
                                            .flex_wrap()
                                            .items_center()
                                            .gap_1p5();
                                        match item.state {
                                            SyncItemState::Queued => {
                                                let hid = item.host_id.clone();
                                                let mid = item.mutation_id.clone();
                                                let mid_dis = mid.clone();
                                                actions = actions.child(
                                                    compact_action(
                                                        "Discard",
                                                        Variant::Quiet,
                                                        move |_| {
                                                            dispatch_sync_action(
                                                                hid.clone(),
                                                                mid.clone(),
                                                                SyncAction::Discard,
                                                            );
                                                        },
                                                    )
                                                    .disabled(move || {
                                                        busy_id.get().as_deref()
                                                            == Some(mid_dis.as_str())
                                                    }),
                                                );
                                            }
                                            SyncItemState::InFlight => {}
                                            SyncItemState::Failed => {
                                                let hid_r = item.host_id.clone();
                                                let mid_r = item.mutation_id.clone();
                                                let mid_r_dis = mid_r.clone();
                                                let hid_d = item.host_id.clone();
                                                let mid_d = item.mutation_id.clone();
                                                let mid_d_dis = mid_d.clone();
                                                actions = actions
                                                    .child(
                                                        compact_action(
                                                            "Retry",
                                                            Variant::Primary,
                                                            move |_| {
                                                                dispatch_sync_action(
                                                                    hid_r.clone(),
                                                                    mid_r.clone(),
                                                                    SyncAction::Retry,
                                                                );
                                                            },
                                                        )
                                                        .disabled(move || {
                                                            busy_id.get().as_deref()
                                                                == Some(mid_r_dis.as_str())
                                                        }),
                                                    )
                                                    .child(
                                                        compact_action(
                                                            "Discard",
                                                            Variant::Quiet,
                                                            move |_| {
                                                                dispatch_sync_action(
                                                                    hid_d.clone(),
                                                                    mid_d.clone(),
                                                                    SyncAction::Discard,
                                                                );
                                                            },
                                                        )
                                                        .disabled(move || {
                                                            busy_id.get().as_deref()
                                                                == Some(mid_d_dis.as_str())
                                                        }),
                                                    );
                                            }
                                            SyncItemState::Conflict => {
                                                let hid_m = item.host_id.clone();
                                                let mid_m = item.mutation_id.clone();
                                                let mid_m_dis = mid_m.clone();
                                                let hid_s = item.host_id.clone();
                                                let mid_s = item.mutation_id.clone();
                                                let mid_s_dis = mid_s.clone();
                                                let mid_e = item.mutation_id.clone();
                                                let mine_title = item
                                                    .mine
                                                    .get("title")
                                                    .or_else(|| item.server.get("title"))
                                                    .cloned()
                                                    .unwrap_or_default();
                                                let mine_body = item
                                                    .mine
                                                    .get("body")
                                                    .or_else(|| item.server.get("body"))
                                                    .cloned()
                                                    .unwrap_or_default();
                                                actions = actions
                                                    .child(
                                                        compact_action(
                                                            "Use mine",
                                                            Variant::Primary,
                                                            move |_| {
                                                                dispatch_sync_action(
                                                                    hid_m.clone(),
                                                                    mid_m.clone(),
                                                                    SyncAction::UseMine,
                                                                );
                                                            },
                                                        )
                                                        .disabled(move || {
                                                            busy_id.get().as_deref()
                                                                == Some(mid_m_dis.as_str())
                                                        }),
                                                    )
                                                    .child(
                                                        compact_action(
                                                            "Use server",
                                                            Variant::Secondary,
                                                            move |_| {
                                                                dispatch_sync_action(
                                                                    hid_s.clone(),
                                                                    mid_s.clone(),
                                                                    SyncAction::Discard,
                                                                );
                                                            },
                                                        )
                                                        .disabled(move || {
                                                            busy_id.get().as_deref()
                                                                == Some(mid_s_dis.as_str())
                                                        }),
                                                    )
                                                    .child(compact_action(
                                                        "Edit merged",
                                                        Variant::Secondary,
                                                        move |_| {
                                                            if editing_id.get_untracked().as_deref()
                                                                == Some(mid_e.as_str())
                                                            {
                                                                editing_id.set(None);
                                                            } else {
                                                                edit_title.set(mine_title.clone());
                                                                edit_body.set(mine_body.clone());
                                                                editing_id.set(Some(mid_e.clone()));
                                                            }
                                                        },
                                                    ));
                                            }
                                        }
                                        row = row.child(actions);

                                        if item.state == SyncItemState::Conflict {
                                            let mid_edit = item.mutation_id.clone();
                                            let hid_save = item.host_id.clone();
                                            let mid_save = item.mutation_id.clone();
                                            let mid_save_dis = mid_save.clone();
                                            let conflict_fields = item.conflict_fields.clone();
                                            row = row.child(when(
                                                move || {
                                                    editing_id.get().as_deref()
                                                        == Some(mid_edit.as_str())
                                                },
                                                move || {
                                                    let mut form = view()
                                                        .p_2p5()
                                                        .rounded(px(RADIUS))
                                                        .bg(SURFACE)
                                                        .border_1()
                                                        .border_color(BORDER)
                                                        .gap_2()
                                                        .min_w(px(0.));
                                                    let has_title = conflict_fields
                                                        .iter()
                                                        .any(|f| f == "title");
                                                    let has_body = conflict_fields
                                                        .iter()
                                                        .any(|f| f == "body");
                                                    if has_title {
                                                        form = form
                                                            .child(muted("Title"))
                                                            .child(
                                                                field(edit_title, "Merged title")
                                                                    .label("Merged title"),
                                                            );
                                                    }
                                                    if has_body {
                                                        form = form
                                                            .child(muted("Body"))
                                                            .child(
                                                                field(edit_body, "Merged body")
                                                                    .label("Merged body"),
                                                            );
                                                    }
                                                    let save_fields = conflict_fields.clone();
                                                    let hid_apply = hid_save.clone();
                                                    let mid_apply = mid_save.clone();
                                                    let mid_apply_dis = mid_save_dis.clone();
                                                    form.child(
                                                        view()
                                                            .flex_row()
                                                            .items_center()
                                                            .gap_1p5()
                                                            .child(
                                                                compact_action(
                                                                    "Save merged",
                                                                    Variant::Primary,
                                                                    move |_| {
                                                                        let mut fields =
                                                                            std::collections::BTreeMap::new();
                                                                        for f in &save_fields {
                                                                            match f.as_str() {
                                                                                "title" => {
                                                                                    fields.insert(
                                                                                        "title".into(),
                                                                                        edit_title.get_untracked(),
                                                                                    );
                                                                                }
                                                                                "body" => {
                                                                                    fields.insert(
                                                                                        "body".into(),
                                                                                        edit_body.get_untracked(),
                                                                                    );
                                                                                }
                                                                                _ => {}
                                                                            }
                                                                        }
                                                                        dispatch_sync_action(
                                                                            hid_apply.clone(),
                                                                            mid_apply.clone(),
                                                                            SyncAction::UseEdited { fields },
                                                                        );
                                                                    },
                                                                )
                                                                .disabled(move || {
                                                                    busy_id.get().as_deref()
                                                                        == Some(
                                                                            mid_apply_dis.as_str(),
                                                                        )
                                                                }),
                                                            )
                                                            .child(compact_action(
                                                                "Cancel",
                                                                Variant::Quiet,
                                                                move |_| editing_id.set(None),
                                                            )),
                                                    )
                                                },
                                            ));
                                        }
                                        body = body.child(row);
                                    }
                                    body
                                })),
                        ),
                ),
        )
}

/// `Settings -> Hosts` section: manage connected `canband` machines (Rename, Enable/Disable, Reconnect, Remove).
pub fn settings_section_view(ui: Ui) -> impl IntoElement {
    let renaming_id = signal(Option::<String>::None);
    let rename_draft = signal(String::new());
    let saved_map = signal(Vec::<SavedHost>::new());

    let reload_saved = move || {
        if let Some(backend) = crate::backend::get().filter(|b| b.has_host_registry()) {
            saved_map.set(backend.saved_hosts());
            let mut sums = backend.host_summaries();
            if let Some(local) = sums
                .iter_mut()
                .find(|h| h.is_local && h.last_synced_at.is_none())
                && let Some(existing) = ui.hosts.with_untracked(|cur| {
                    cur.iter()
                        .find(|h| h.is_local && h.last_synced_at.is_some())
                        .cloned()
                })
            {
                local.active_agents = existing.active_agents;
                local.project_count = existing.project_count;
                local.last_synced_at = existing.last_synced_at;
            }
            ui.hosts.set(sums);
        }
    };
    reload_saved();

    settings_section(
        "Hosts",
        format!(
            "Connect Canban to remote Linux or macOS `canband` services over Tailscale or SSH while keeping {} active simultaneously.",
            crate::platform::LOCAL_HOST_NAME
        ),
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
                    if h.is_updating() {
                        controls = controls.child(
                            compact_action("Updating…", Variant::Secondary, |_| {})
                                .disabled(|| true),
                        );
                    } else {
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
                }
                controls = controls
                    .child(
                        compact_action(
                            if is_enabled { "Disable" } else { "Enable" },
                            Variant::Quiet,
                            move |_| {
                                if let Some(backend) =
                                    crate::backend::get().filter(|b| b.has_host_registry())
                                {
                                    if let Err(e) =
                                        backend.set_host_enabled(&hid_toggle, !is_enabled)
                                    {
                                        ui.operation_error.set(format!("{e:#}"));
                                    } else {
                                        reload_saved();
                                        refresh(ui);
                                    }
                                }
                            },
                        )
                        .disabled({
                            let updating = h.is_updating();
                            move || updating
                        }),
                    )
                    .child(compact_action("Rename", Variant::Quiet, move |_| {
                        rename_draft.set(cur_name.clone());
                        renaming_id.set(Some(hid_edit.clone()));
                    }))
                    .child(compact_action("Remove", Variant::Danger, move |_| {
                        if let Some(backend) =
                            crate::backend::get().filter(|b| b.has_host_registry())
                        {
                            if let Err(e) = backend.remove_host(&hid_remove) {
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
                                .child(host_status_badge(h)),
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
                            if let Some(backend) =
                                crate::backend::get().filter(|b| b.has_host_registry())
                            {
                                match backend.rename_host(&save_id, &next) {
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
                pending_mutations: 0,
                sync_conflicts: 0,
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
                pending_mutations: 0,
                sync_conflicts: 0,
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
            texts.iter().any(|t| t.contains("Home Linux is offline")
                && t.contains("edits sync when it reconnects")),
            "expected offline cached-edit banner for Home Linux, got {texts:?}"
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
                pending_mutations: 0,
                sync_conflicts: 0,
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
                pending_mutations: 0,
                sync_conflicts: 0,
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

    #[test]
    fn updating_host_renders_progress_status_and_replaces_update_action_across_surfaces() {
        #[component]
        fn ProbeUpdatingHosts() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut snapshot = Snapshot::default();
            snapshot.projects.push(Project {
                id: "home-linux:proj-remote".into(),
                name: "Backend Worker".into(),
                directory: "/home/jjy/backend".into(),
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
                pending_mutations: 0,
                sync_conflicts: 0,
            };
            let updating_summary = HostSummary {
                id: "home-linux".into(),
                name: "Home Linux".into(),
                is_local: false,
                transport_label: "Tailscale / TCP".into(),
                endpoint_label: "100.64.0.10:7680".into(),
                ssh_target: Some("jjy@100.64.0.10".into()),
                server_id: Some("srv-home".into()),
                connection: HostConnectionState::Updating {
                    version: "20261116-1".into(),
                },
                active_agents: 0,
                max_agents: 8,
                project_count: 1,
                reconnect_count: 0,
                last_synced_at: Some(1_700_000_000_000),
                pending_mutations: 0,
                sync_conflicts: 0,
            };
            snapshot
                .entity_hosts
                .insert("home-linux:proj-remote".into(), "home-linux".into());
            snapshot
                .host_summaries
                .insert("local".into(), local_summary.clone());
            snapshot
                .host_summaries
                .insert("home-linux".into(), updating_summary.clone());
            ui.hosts.set(vec![local_summary, updating_summary]);
            ui.snapshot.set(snapshot);
            ui.project.set("home-linux:proj-remote".into());
            ui.utility_popup.set("hosts".into());
            view()
                .w(px(1200.))
                .h(px(1100.))
                .child(trigger_pill(ui))
                .child(offline_banner(ui))
                .child(settings_section_view(ui))
                .child(popover(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(1100.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbeUpdatingHosts>());
        let texts = collect_mounted_texts(core.commit().expect("mount batch"));

        let status_badges = texts
            .iter()
            .filter(|t| *t == "Updating to 20261116-1…")
            .count();
        assert!(
            status_badges >= 2,
            "expected `Updating to 20261116-1…` status in Connected Hosts popover and Settings -> Hosts, got {texts:?}"
        );
        let busy_buttons = texts.iter().filter(|t| *t == "Updating…").count();
        assert!(
            busy_buttons >= 3,
            "expected disabled `Updating…` action indicator in popover, settings, and banner, got {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t == "Update Daemon"),
            "did not expect clickable `Update Daemon` while host is already updating, got {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Home Linux is updating (Updating to 20261116-1…)")),
            "expected updating banner copy for Home Linux, got {texts:?}"
        );
    }

    #[test]
    fn optimistic_mark_host_connection_transitions_mismatch_to_updating() {
        #[component]
        fn ProbeOptimisticUpdate() -> impl IntoElement {
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
                pending_mutations: 0,
                sync_conflicts: 0,
            };
            let mismatch = HostSummary {
                id: "home-linux".into(),
                name: "Home Linux".into(),
                is_local: false,
                transport_label: "Tailscale / TCP".into(),
                endpoint_label: "100.64.0.10:7680".into(),
                ssh_target: None,
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
                pending_mutations: 0,
                sync_conflicts: 0,
            };
            let mut snap = Snapshot::default();
            snap.host_summaries
                .insert("local".into(), local_summary.clone());
            snap.host_summaries
                .insert("home-linux".into(), mismatch.clone());
            ui.hosts.set(vec![local_summary, mismatch.clone()]);
            ui.snapshot.set(snap);
            ui.utility_popup.set("hosts".into());

            // Trigger optimistic transition directly.
            mark_host_connection(
                ui,
                &mismatch.id,
                HostConnectionState::Updating {
                    version: String::new(),
                },
            );

            view().w(px(1200.)).h(px(800.)).child(popover(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(800.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbeOptimisticUpdate>());
        let texts = collect_mounted_texts(core.commit().expect("mount batch"));

        assert!(
            texts.iter().any(|t| t == "Updating daemon…"),
            "expected optimistic `Updating daemon…` status in Connected Hosts popover, got {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t == "Update Daemon"),
            "expected `Update Daemon` button hidden during optimistic update, got {texts:?}"
        );
    }

    #[test]
    fn hosts_popover_keeps_stable_action_nodes_when_only_rtt_polls_change() {
        use nsh_core::{EventKind, EventPayload, ROOT_ID};

        fn make_hosts(rtt_ms: u64, synced_at: u64) -> Vec<HostSummary> {
            vec![
                HostSummary {
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
                    project_count: 1,
                    reconnect_count: 0,
                    last_synced_at: Some(synced_at),
                    pending_mutations: 0,
                    sync_conflicts: 0,
                },
                HostSummary {
                    id: "home-linux".into(),
                    name: "Home Linux".into(),
                    is_local: false,
                    transport_label: "Tailscale / TCP".into(),
                    endpoint_label: "100.64.0.10:7680".into(),
                    ssh_target: None,
                    server_id: Some("srv-home".into()),
                    connection: HostConnectionState::Connected {
                        rtt_ms,
                        api_version: canban_updater::DESKTOP_API,
                        version: "0.4.2".into(),
                        platform: "linux-x86_64".into(),
                    },
                    active_agents: 0,
                    max_agents: 8,
                    project_count: 1,
                    reconnect_count: 0,
                    last_synced_at: Some(synced_at),
                    pending_mutations: 0,
                    sync_conflicts: 0,
                },
            ]
        }

        #[component]
        fn ProbeHostsStability() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.hosts.set(make_hosts(12, 1_000));
            ui.utility_popup.set("hosts".into());
            view()
                .w(px(1200.))
                .h(px(800.))
                .on_press(move |_: &()| {
                    ui.hosts.set(make_hosts(47, 2_500));
                })
                .child(trigger_pill(ui))
                .child(popover(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(800.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbeHostsStability>());
        let initial = core.commit().expect("initial mount");
        let host = initial
            .items
            .iter()
            .find_map(|item| match item {
                MountItem::Insert { parent, child, .. } if *parent == ROOT_ID => Some(*child),
                _ => None,
            })
            .expect("root host node");
        let manage_id = initial
            .items
            .iter()
            .find_map(|item| match item {
                MountItem::Text { id, text, .. } if text.text == "Manage Hosts…" => Some(*id),
                _ => None,
            })
            .expect("expected `Manage Hosts…` in initial mount");
        let settings_id = initial
            .items
            .iter()
            .find_map(|item| match item {
                MountItem::Text { id, text, .. } if text.text == "Settings" => Some(*id),
                _ => None,
            })
            .expect("expected `Settings` in initial mount");

        assert!(core.component_event(host, EventKind::Press, &EventPayload::None));
        if let Some(update) = core.commit() {
            assert!(
                !update
                    .items
                    .iter()
                    .any(|item| matches!(item, MountItem::Delete { id } if *id == manage_id || *id == settings_id)),
                "Manage Hosts… ({manage_id:?}) and Settings ({settings_id:?}) must not be deleted/remounted when only rtt_ms/last_synced_at change: {:?}",
                update.items
            );
            assert!(
                !update.items.iter().any(|item| matches!(
                    item,
                    MountItem::Text { text, .. } if text.text == "Manage Hosts…" || text.text == "Settings"
                )),
                "Manage Hosts… and Settings must not be re-emitted as new Text nodes when only rtt_ms/last_synced_at change"
            );
        }
    }

    #[test]
    fn sync_modal_renders_failed_and_conflict_actions() {
        #[component]
        fn ProbeSyncModal() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut mine = std::collections::BTreeMap::new();
            mine.insert("title".into(), "Offline edited title".into());
            let mut server = std::collections::BTreeMap::new();
            server.insert("title".into(), "Server concurrent title".into());
            ui.sync_preview.set(vec![
                SyncItemView {
                    host_id: "home-linux".into(),
                    host_name: "Home Linux".into(),
                    mutation_id: "m-fail".into(),
                    card_id: "c1".into(),
                    card_title: "Move Stage Offline".into(),
                    kind: "Move Card".into(),
                    state: SyncItemState::Failed,
                    message: "Stage no longer exists".into(),
                    conflict_fields: Vec::new(),
                    mine: std::collections::BTreeMap::new(),
                    server: std::collections::BTreeMap::new(),
                },
                SyncItemView {
                    host_id: "home-linux".into(),
                    host_name: "Home Linux".into(),
                    mutation_id: "m-conflict".into(),
                    card_id: "c2".into(),
                    card_title: "Autonomous Sync Spec".into(),
                    kind: "Edit Card".into(),
                    state: SyncItemState::Conflict,
                    message: "Conflict on title".into(),
                    conflict_fields: vec!["title".into()],
                    mine,
                    server,
                },
            ]);
            ui.utility_popup.set("sync".into());
            view().w(px(900.)).h(px(700.)).child(sync_modal(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(900.), px(700.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbeSyncModal>());
        let texts = collect_mounted_texts(core.commit().expect("mount sync modal"));
        for expected in ["Retry", "Discard", "Use mine", "Use server", "Edit merged"] {
            assert!(
                texts.iter().any(|t| t == expected),
                "expected `{expected}` button in sync modal, got {texts:?}"
            );
        }
    }
}
