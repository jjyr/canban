//! Single-address QR / URI pairing screen & confirmation sheet with transport security badges.
use super::{
    MobileState,
    chrome::{is_ios, security_badge, trigger_haptic},
};
use crate::presenter::*;
use canban_client::{AddrProbeResult, PairingInvite, TransportSecurity, parse_pairing_invite};
use canban_ui_kit::{tokens::*, *};
use nsh_core::{Haptic, HapticNotification, prelude::*};
use std::time::Duration;

/// Live single-address reachability probe state during pairing confirmation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProbeState {
    #[default]
    Idle,
    Probing(String),
    Result(AddrProbeResult),
}

/// Triggers an asynchronous single-address reachability probe for `invite.addr`.
pub(crate) fn start_probe(ui: Ui, state: MobileState, addr: String) {
    if addr.is_empty() {
        return;
    }
    state.pairing_probe.set(ProbeState::Probing(addr.clone()));
    state.pairing_error.set(String::new());
    let expected = addr.clone();
    ui.spawn(async move {
        let client_opt = crate::backend::remote_client();
        let res = blocking(move || {
            if let Some(client) = client_opt {
                client.block_on(canban_client::probe_addr(&addr, Duration::from_secs(3)))
            } else {
                canban_client::probe_addr_blocking(&addr, Duration::from_secs(3))
            }
        })
        .await;
        let current = state.pairing_probe.try_get();
        if matches!(current, Some(ProbeState::Probing(ref cur)) if cur == &expected)
            && let Ok(result) = res
        {
            state.pairing_probe.try_set(ProbeState::Result(result));
        }
    });
}

fn connect_invite(ui: Ui, state: MobileState, invite: PairingInvite, name_override: String) {
    if state.pairing_busy.get_untracked() {
        return;
    }
    state.pairing_busy.set(true);
    state.pairing_error.set(String::new());
    trigger_haptic(Haptic::Selection);

    let Some(client) = crate::backend::remote_client() else {
        // Deterministic Gallery preview fallback without live client.
        state.pairing_busy.set(false);
        let label = if name_override.trim().is_empty() {
            invite.host_name.as_deref().unwrap_or(&invite.addr)
        } else {
            name_override.trim()
        };
        ui.notify(format!(
            "Gallery preview · paired {label} ({})",
            invite.security.label()
        ));
        state.close_sheet();
        return;
    };

    ui.spawn(async move {
        let label_opt = (!name_override.trim().is_empty()).then_some(name_override);
        let client_call = client.clone();
        let outcome = blocking(move || {
            let summary = client_call
                .block_on(client_call.pair_with_invite(&invite, label_opt.as_deref()))?;
            let sums = client_call.host_summaries();
            let snap = client_call.merged_snapshot()?;
            anyhow::Ok((summary, sums, snap))
        })
        .await;

        state.pairing_busy.try_set(false);
        match outcome {
            Ok(Ok((summary, sums, snap))) => {
                let _ = ui.hosts.try_set(sums);
                ui.accept_snapshot(snap);
                if ui.project.get_untracked().is_empty()
                    && let Some(first_proj) = ui
                        .snapshot
                        .with_untracked(|s| s.projects.first().map(|p| p.id.clone()))
                {
                    let _ = ui.project.try_set(first_proj);
                }
                trigger_haptic(Haptic::Notification(HapticNotification::Success));
                ui.notify(format!("Connected to {}", summary.name));
                state.close_sheet();
                crate::presenter::refresh(ui);
            }
            Ok(Err(e)) => {
                trigger_haptic(Haptic::Notification(HapticNotification::Error));
                state.pairing_error.try_set(format!("{e:#}"));
            }
            Err(e) => {
                state.pairing_error.try_set(e.to_string());
            }
        }
    });
}

/// Interactive single-address pairing form & confirmation card shared between
/// the first-run onboarding screen and `MobileSheet::Pairing`.
pub(crate) fn pairing_panel(ui: Ui, state: MobileState) -> impl IntoElement {
    let custom_name = signal(String::new());
    let parsed_invite = computed(move || {
        let raw = state.pairing_uri_input.get();
        parse_pairing_invite(&raw)
    });

    // Trigger automatic probe whenever parsed invite address changes.
    let last_probed_addr = stored(String::new());
    effect(move || {
        let next_addr = parsed_invite.get().map(|inv| inv.addr).unwrap_or_default();
        if next_addr.is_empty() {
            last_probed_addr.replace(String::new());
            state.pairing_probe.set(ProbeState::Idle);
            return;
        }
        if last_probed_addr.get() != next_addr {
            last_probed_addr.replace(next_addr.clone());
            // Preserve pre-populated Gallery fixture result if already targeting this addr.
            let already_for_addr = matches!(
                state.pairing_probe.get_untracked(),
                ProbeState::Result(ref r) if r.addr == next_addr
            );
            if !already_for_addr {
                start_probe(ui, state, next_addr);
            }
        }
    });

    view()
        .w_full()
        .gap_3()
        .child(
            view()
                .w_full()
                .gap_1()
                .child(heading("Connect to Canban host"))
                .child(muted(
                    "Scan a pairing QR code with your iPhone Camera (opens `canban://` directly), or paste the pairing URI below.",
                )),
        )
        .child(
            view()
                .w_full()
                .gap_1p5()
                .child(label("Pairing URI"))
                .child(
                    view()
                        .w_full()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            field(
                                state.pairing_uri_input,
                                "canban://host:7680?name=...#code",
                            )
                            .flex_1()
                            .min_w(px(0.)),
                        )
                        .child(compact_action("Paste URI", Variant::Secondary, move |_| {
                            trigger_haptic(Haptic::Selection);
                            ui.spawn(async move {
                                let clip = clipboard_string().await;
                                let trimmed = clip.trim().to_owned();
                                if trimmed.is_empty() {
                                    state.pairing_error.try_set(
                                        "Clipboard is empty. Copy a `canban://` link from Settings -> Remote Access on your desktop.".into(),
                                    );
                                } else {
                                    state.pairing_uri_input.try_set(trimmed);
                                    state.pairing_error.try_set(String::new());
                                }
                            });
                        })),
                ),
        )
        .child(dynamic(move || {
            let Some(invite) = parsed_invite.get() else {
                let raw_empty = state.pairing_uri_input.with(|s| s.trim().is_empty());
                if raw_empty {
                    return panel()
                        .p_3()
                        .gap_1()
                        .child(label("Waiting for pairing URI"))
                        .child(muted(
                            "On your Mac or Linux host, open Settings -> Remote Access / Pair Mobile to show a QR code or copy a `canban://` link.",
                        ))
                        .into_element();
                }
                return notice(
                    "Invalid pairing URI. Expected `canban://<addr>?name=...#<secret>`.",
                    Tone::Attention,
                )
                .into_element();
            };

            let display_name = invite
                .host_name
                .clone()
                .unwrap_or_else(|| invite.addr.clone());
            let probe = state.pairing_probe.get();
            let probe_addr_retry = invite.addr.clone();
            let connect_inv = invite.clone();
            let is_tailscale = invite.security == TransportSecurity::Tailscale;

            let probe_row = match probe {
                ProbeState::Idle => view().child(muted("Ready to verify connection…")),
                ProbeState::Probing(ref addr) => view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(loading_spinner(14., ACCENT))
                    .child(muted(format!("Checking reachability of {addr}…"))),
                ProbeState::Result(ref res) if res.reachable => view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(status(
                        format!(
                            "Reachable · {}ms",
                            res.rtt_ms.unwrap_or(0)
                        ),
                        Tone::Success,
                    ))
                    .child(compact_action("Recheck", Variant::Quiet, move |_| {
                        start_probe(ui, state, probe_addr_retry.clone());
                    })),
                ProbeState::Result(ref res) => view()
                    .w_full()
                    .gap_2()
                    .child(notice(
                        format!(
                            "Cannot reach {} ({})",
                            res.addr,
                            res.error.as_deref().unwrap_or("unreachable")
                        ),
                        Tone::Attention,
                    ))
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(compact_action("Retry", Variant::Secondary, move |_| {
                                start_probe(ui, state, probe_addr_retry.clone());
                            }))
                            .children(is_tailscale.then(|| {
                                compact_action("Open Tailscale", Variant::Secondary, move |_| {
                                    ui.spawn(async {
                                        let _ = open_url("tailscale://").await;
                                    });
                                })
                            })),
                    ),
            };

            panel()
                .p_3()
                .gap_3()
                .child(
                    view()
                        .w_full()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(label(display_name).font_weight(FontWeight::SEMIBOLD))
                        .child(security_badge(invite.security)),
                )
                .child(muted(invite.addr.clone()).font_family(MONO_FONT))
                .child(muted(invite.security.hint()))
                .child(
                    view()
                        .w_full()
                        .gap_1()
                        .child(muted("Label (optional)"))
                        .child(field(
                            custom_name,
                            invite.host_name.as_deref().unwrap_or("My Mac / Linux"),
                        )),
                )
                .child(probe_row)
                .child(
                    action(
                        move || {
                            if state.pairing_busy.get() {
                                "Connecting…"
                            } else {
                                "Connect"
                            }
                        },
                        Variant::Primary,
                        move |_| connect_invite(ui, state, connect_inv.clone(), custom_name.get_untracked()),
                    )
                    .disabled(move || state.pairing_busy.get()),
                )
                .into_element()
        }))
        .child(when(
            move || !state.pairing_error.with(String::is_empty),
            move || notice(move || state.pairing_error.get(), Tone::Danger),
        ))
}

/// Fullscreen first-run onboarding screen shown when 0 hosts are paired.
pub(crate) fn onboarding_screen(ui: Ui, state: MobileState) -> impl IntoElement {
    view()
        .size_full()
        .flex_col()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .children((!is_ios()).then(|| {
            view()
                .w_full()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .child(heading("Canban"))
        }))
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                view()
                    .w_full()
                    .p_4()
                    .gap_4()
                    .child(pairing_panel(ui, state)),
            ),
        )
}
