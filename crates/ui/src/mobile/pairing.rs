//! Mobile-first QR / URI pairing screen & confirmation sheet with transport security badges.
use super::{
    MobileState,
    chrome::{
        MOBILE_CAPTION, MOBILE_HEADLINE, MOBILE_HERO_TITLE, MOBILE_RADIUS, grouped_card,
        is_mobile_os, mobile_button, mobile_field, mobile_footnote, mobile_inline_button,
        mobile_subhead, section_header, security_badge, trigger_haptic,
    },
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

/// Interactive single-address pairing flow & confirmation card shared between
/// the first-run onboarding screen and `MobileSheet::Pairing`.
pub(crate) fn pairing_panel(ui: Ui, state: MobileState) -> impl IntoElement {
    let custom_name = signal(String::new());
    let show_manual = signal(false);
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
            let already_for_addr = matches!(
                state.pairing_probe.get_untracked(),
                ProbeState::Result(ref r) if r.addr == next_addr
            );
            if !already_for_addr {
                start_probe(ui, state, next_addr);
            }
        }
    });

    let paste_btn = move |primary: bool| {
        mobile_button(
            "Paste Pairing Link",
            if primary {
                Variant::Primary
            } else {
                Variant::Secondary
            },
            move |_| {
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
            },
        )
    };

    view()
        .w_full()
        .gap_4()
        .child(dynamic(move || {
            let invite_opt = parsed_invite.get();
            if invite_opt.is_none() {
                let raw_empty = state.pairing_uri_input.with(|s| s.trim().is_empty());
                return view()
                    .w_full()
                    .gap_3()
                    .child(paste_btn(true))
                    .child(
                        press_target()
                            .w_full()
                            .min_h(px(40.))
                            .items_center()
                            .justify_center()
                            .active(|s| s.opacity(0.65))
                            .child(
                                text(move || {
                                    if show_manual.get() {
                                        "Hide manual entry"
                                    } else {
                                        "Enter URI manually"
                                    }
                                })
                                .text_size(px(14.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ACCENT),
                            )
                            .on_press(move |_: &()| {
                                trigger_haptic(Haptic::Selection);
                                show_manual.update(|v| *v = !*v);
                            }),
                    )
                    .child(when(
                        move || show_manual.get() || !raw_empty,
                        move || {
                            view()
                                .w_full()
                                .gap_2()
                                .child(mobile_field(
                                    state.pairing_uri_input,
                                    "canban://host:7680?name=...#code",
                                ))
                                .children((!raw_empty).then(|| {
                                    notice(
                                        "Invalid pairing URI. Expected `canban://<addr>?name=...#<secret>`.",
                                        Tone::Attention,
                                    )
                                }))
                        },
                    ))
                    .into_element();
            }

            let invite = invite_opt.unwrap();
            let display_name = invite
                .host_name
                .clone()
                .unwrap_or_else(|| invite.addr.clone());
            let probe = state.pairing_probe.get();
            let probe_addr_retry = invite.addr.clone();
            let connect_inv = invite.clone();
            let is_tailscale = invite.security == TransportSecurity::Tailscale;

            let probe_row: AnyElement = match probe {
                ProbeState::Idle => view()
                    .w_full()
                    .px_4()
                    .py_3()
                    .child(mobile_footnote("Ready to verify connection…"))
                    .into_element(),
                ProbeState::Probing(ref addr) => view()
                    .w_full()
                    .px_4()
                    .py_3()
                    .flex_row()
                    .items_center()
                    .gap_2p5()
                    .child(loading_spinner(15., ACCENT))
                    .child(mobile_subhead(format!("Checking {addr}…")))
                    .into_element(),
                ProbeState::Result(ref res) if res.reachable => view()
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(status(
                        format!("Reachable · {}ms", res.rtt_ms.unwrap_or(0)),
                        Tone::Success,
                    ))
                    .child(
                        press_target()
                            .min_h(px(36.))
                            .px_2p5()
                            .items_center()
                            .justify_center()
                            .child(
                                text("Recheck")
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ACCENT),
                            )
                            .on_press(move |_: &()| {
                                trigger_haptic(Haptic::Selection);
                                start_probe(ui, state, probe_addr_retry.clone());
                            }),
                    )
                    .into_element(),
                ProbeState::Result(ref res) => view()
                    .w_full()
                    .p_3p5()
                    .gap_2p5()
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
                            .w_full()
                            .flex_row()
                            .gap_2()
                            .child(
                                mobile_inline_button("Retry", Variant::Secondary, move |_| {
                                    start_probe(ui, state, probe_addr_retry.clone());
                                })
                                .flex_1(),
                            )
                            .children(is_tailscale.then(|| {
                                mobile_inline_button(
                                    "Open Tailscale",
                                    Variant::Secondary,
                                    move |_| {
                                        ui.spawn(async {
                                            let _ = open_url("tailscale://").await;
                                        });
                                    },
                                )
                                .flex_1()
                            })),
                    )
                    .into_element(),
            };

            let summary_row = view()
                .w_full()
                .px_4()
                .py_3p5()
                .gap_1p5()
                .child(
                    view()
                        .w_full()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            text(display_name.clone())
                                .text_size(px(MOBILE_HEADLINE))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(TEXT),
                        )
                        .child(security_badge(invite.security)),
                )
                .child(
                    text(invite.addr.clone())
                        .text_size(px(13.5))
                        .font_family(MONO_FONT)
                        .text_color(MUTED),
                )
                .child(mobile_footnote(invite.security.hint()))
                .into_element();

            let label_row = view()
                .w_full()
                .px_4()
                .py_3()
                .gap_1p5()
                .child(
                    text("LABEL (OPTIONAL)")
                        .text_size(px(MOBILE_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(MUTED),
                )
                .child(mobile_field(
                    custom_name,
                    invite.host_name.as_deref().unwrap_or("My Mac / Linux"),
                ))
                .into_element();

            view()
                .w_full()
                .gap_4()
                .child(
                    view()
                        .w_full()
                        .child(section_header("CONFIRM MACHINE", None))
                        .child(grouped_card(vec![summary_row, probe_row, label_row])),
                )
                .child(
                    mobile_button(
                        move || {
                            if state.pairing_busy.get() {
                                "Connecting…".to_owned()
                            } else {
                                format!("Connect to {display_name}")
                            }
                        },
                        Variant::Primary,
                        move |_| {
                            connect_invite(
                                ui,
                                state,
                                connect_inv.clone(),
                                custom_name.get_untracked(),
                            );
                        },
                    )
                    .disabled(move || state.pairing_busy.get()),
                )
                .child(
                    view()
                        .w_full()
                        .flex_row()
                        .gap_2()
                        .child(paste_btn(false).flex_1())
                        .child(
                            mobile_button("Clear", Variant::Quiet, move |_| {
                                state.pairing_uri_input.set(String::new());
                                state.pairing_probe.set(ProbeState::Idle);
                                state.pairing_error.set(String::new());
                            })
                            .flex_1(),
                        ),
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
    scroll().size_full().bg(BACKGROUND).child(
        view()
            .w_full()
            .px_5()
            .pt(move || {
                px(if is_mobile_os() {
                    safe_area().top.0 + 36.0
                } else {
                    36.0
                })
            })
            .pb(move || {
                px(if is_mobile_os() {
                    safe_area().bottom.0 + 36.0
                } else {
                    36.0
                })
            })
            .gap_6()
            .child(
                view()
                    .w_full()
                    .items_center()
                    .gap_3()
                    .pt_2()
                    .child(
                        view()
                            .w(px(64.))
                            .h(px(64.))
                            .rounded(px(18.))
                            .bg(SELECTED)
                            .border_1()
                            .border_color(ACCENT)
                            .items_center()
                            .justify_center()
                            .child(icon_tinted(Icon::Board, ACCENT).w(px(30.)).h(px(30.))),
                    )
                    .child(
                        text("Connect to Canban")
                            .text_size(px(MOBILE_HERO_TITLE))
                            .font_weight(FontWeight::BOLD)
                            .letter_spacing(px(-0.4))
                            .text_color(TEXT)
                            .text_center(),
                    )
                    .child(
                        view().w_full().max_w(px(320.)).items_center().child(
                            mobile_subhead(
                                "Scan the QR code in desktop Settings -> Remote Access with your phone Camera, or paste a pairing link below.",
                            )
                            .line_height(px(21.))
                            .text_center(),
                        ),
                    ),
            )
            .child(pairing_panel(ui, state))
            .children(
                state.pairing_error.with_untracked(|_| false).then(|| {
                    view().rounded(px(MOBILE_RADIUS))
                }),
            ),
    )
}
