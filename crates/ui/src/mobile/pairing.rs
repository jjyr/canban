//! Mobile-first QR / URI / Tailscale pairing screen, confirmation sheet, and Tailnet host discovery.
use super::{
    MobileSheet, MobileState,
    chrome::{
        MOBILE_CAPTION, MOBILE_HEADLINE, MOBILE_HERO_TITLE, MOBILE_RADIUS, empty_state,
        grouped_card, icon_badge, is_mobile_os, mobile_button, mobile_field, mobile_footnote,
        mobile_inline_button, mobile_subhead, mobile_title, section_header, security_badge,
        trigger_haptic,
    },
};
use crate::presenter::*;
use canban_client::{
    AddrProbeResult, PairingInvite, TransportSecurity, parse_pairing_invite,
    parse_tailscale_host_input,
};
use canban_types::{DiscoveredCanbanHost, is_tailscale_auth};
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
                run_tailnet_discovery(ui, state, Some(summary.id), true);
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

const INVALID_PAIRING_INPUT_HINT: &str = "Enter a Tailscale address (host.ts.net or 100.x.y.z) or a `canban://<addr>?name=...#<secret>` pairing link.";

fn parse_pairing_input(raw: &str) -> Option<PairingInvite> {
    parse_pairing_invite(raw).or_else(|| parse_tailscale_host_input(raw))
}

/// Interactive single-address pairing flow & confirmation card shared between
/// the first-run onboarding screen and `MobileSheet::Pairing`.
pub(crate) fn pairing_panel(ui: Ui, state: MobileState) -> impl IntoElement {
    let custom_name = signal(String::new());
    let initial_raw = state.pairing_uri_input.get_untracked();
    let initial_needs_manual =
        !initial_raw.trim().is_empty() && parse_pairing_input(&initial_raw).is_none();
    let manual_draft = signal(initial_raw);
    let manual_validation_error = signal(String::new());
    let error_for_draft: Stored<Option<String>> = stored(None);
    let show_manual = signal(initial_needs_manual);
    let parsed_invite = computed(move || {
        let raw = state.pairing_uri_input.get();
        parse_pairing_input(&raw)
    });
    let has_invite = computed(move || parsed_invite.with(Option::is_some));

    // Sync external URI updates (QR deep links, Paste, or discovery `URI…` prefill) into the
    // manual draft without ever remounting the input during manual typing.
    effect(move || {
        let raw = state.pairing_uri_input.get();
        if raw.trim().is_empty() {
            return;
        }
        if parse_pairing_input(&raw).is_some() {
            if manual_draft.with_untracked(|cur| cur != &raw) {
                manual_draft.set(raw);
            }
            error_for_draft.replace(None);
            if !manual_validation_error.with_untracked(String::is_empty) {
                manual_validation_error.set(String::new());
            }
        } else {
            if manual_draft.with_untracked(|cur| cur != &raw) {
                manual_draft.set(raw);
            }
            if !show_manual.get_untracked() {
                show_manual.set(true);
            }
        }
    });

    // Clear stale validation feedback once the user edits the manual draft.
    effect(move || {
        let draft = manual_draft.get();
        let matches_failed =
            error_for_draft.with(|failed| failed.as_deref() == Some(draft.as_str()));
        if !matches_failed && !manual_validation_error.with_untracked(String::is_empty) {
            error_for_draft.replace(None);
            manual_validation_error.set(String::new());
        }
    });

    // Trigger automatic probe whenever a committed invite address changes.
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

    let submit_manual = move || {
        let raw = manual_draft.get_untracked();
        let trimmed = raw.trim().to_owned();
        if trimmed.is_empty() || parse_pairing_input(&trimmed).is_none() {
            error_for_draft.replace(Some(raw));
            manual_validation_error.set(INVALID_PAIRING_INPUT_HINT.to_owned());
            return;
        }
        trigger_haptic(Haptic::Selection);
        batch(move || {
            error_for_draft.replace(None);
            manual_validation_error.set(String::new());
            state.pairing_error.set(String::new());
            state.pairing_uri_input.set(trimmed);
        });
    };

    let paste_btn = move |label: &'static str, primary: bool| {
        mobile_button(
            label,
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
                        let _ = state.pairing_error.try_set(
                            "Clipboard is empty. Enter a Tailscale address (host.ts.net or 100.x.y.z) or copy a `canban://` link from Settings -> Remote Access.".into(),
                        );
                    } else if parse_pairing_input(&trimmed).is_some() {
                        error_for_draft.replace(None);
                        let _ = manual_draft.try_set(trimmed.clone());
                        let _ = manual_validation_error.try_set(String::new());
                        let _ = state.pairing_error.try_set(String::new());
                        let _ = state.pairing_uri_input.try_set(trimmed);
                    } else {
                        error_for_draft.replace(Some(trimmed.clone()));
                        let _ = manual_draft.try_set(trimmed);
                        let _ =
                            manual_validation_error.try_set(INVALID_PAIRING_INPUT_HINT.to_owned());
                        let _ = show_manual.try_set(true);
                        let _ = state.pairing_error.try_set(String::new());
                        let _ = state.pairing_uri_input.try_set(String::new());
                    }
                });
            },
        )
    };

    view()
        .w_full()
        .gap_4()
        .child(
            when(move || !has_invite.get(), move || {
                view()
                    .w_full()
                    .gap_3()
                    .child(
                        when(
                            move || show_manual.get(),
                            move || paste_btn("Paste Pairing Link", false),
                        )
                        .otherwise(move || paste_btn("Paste Pairing Link", true)),
                    )
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
                                        "Enter Tailscale address or URI"
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
                        move || show_manual.get(),
                        move || {
                            view()
                                .w_full()
                                .gap_2p5()
                                .child(
                                    mobile_field(
                                        manual_draft,
                                        "host.ts.net, 100.x.y.z, or canban://...#code",
                                    )
                                    .keyboard_type(KeyboardType::Url)
                                    .auto_capitalize(AutoCapitalize::None)
                                    .auto_correct(false)
                                    .return_key_type(ReturnKeyType::Go)
                                    .on_submit(move |_: &str| submit_manual()),
                                )
                                .child(when(
                                    move || !manual_validation_error.with(String::is_empty),
                                    move || {
                                        notice(
                                            move || manual_validation_error.get(),
                                            Tone::Attention,
                                        )
                                    },
                                ))
                                .child(
                                    mobile_button("Check Address", Variant::Primary, move |_| {
                                        submit_manual();
                                    })
                                    .disabled(move || manual_draft.with(|s| s.trim().is_empty())),
                                )
                        },
                    ))
            })
            .otherwise(move || {
                let summary_row = dynamic(move || {
                    let Some(invite) = parsed_invite.get() else {
                        return view().into_element();
                    };
                    let display_name = invite
                        .host_name
                        .clone()
                        .unwrap_or_else(|| invite.addr.clone());
                    view()
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
                                    text(display_name)
                                        .text_size(px(MOBILE_HEADLINE))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(TEXT),
                                )
                                .child(security_badge(invite.security)),
                        )
                        .child(
                            text(invite.addr)
                                .text_size(px(13.5))
                                .font_family(MONO_FONT)
                                .text_color(MUTED),
                        )
                        .child(mobile_footnote(if is_tailscale_auth(&invite.secret) {
                            "Tailscale — automatic same-account sign-in (managed in Tailscale Admin)"
                        } else {
                            invite.security.hint()
                        }))
                        .into_element()
                });

                let probe_row = dynamic(move || {
                    let Some(invite) = parsed_invite.get() else {
                        return view().into_element();
                    };
                    let probe = state.pairing_probe.get();
                    let probe_addr_retry = invite.addr;
                    let is_tailscale = invite.security == TransportSecurity::Tailscale;
                    match probe {
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
                                        mobile_inline_button(
                                            "Retry",
                                            Variant::Secondary,
                                            move |_| {
                                                start_probe(ui, state, probe_addr_retry.clone());
                                            },
                                        )
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
                    }
                });

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
                    .child(mobile_field(custom_name, "My Mac / Linux").placeholder(move || {
                        parsed_invite.with(|inv| {
                            inv.as_ref()
                                .and_then(|i| i.host_name.clone())
                                .unwrap_or_else(|| "My Mac / Linux".to_owned())
                        })
                    }))
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
                                    let display_name = parsed_invite.with(|inv| {
                                        inv.as_ref()
                                            .map(|i| {
                                                i.host_name
                                                    .clone()
                                                    .unwrap_or_else(|| i.addr.clone())
                                            })
                                            .unwrap_or_default()
                                    });
                                    format!("Connect to {display_name}")
                                }
                            },
                            Variant::Primary,
                            move |_| {
                                if let Some(invite) = parsed_invite.get_untracked() {
                                    connect_invite(
                                        ui,
                                        state,
                                        invite,
                                        custom_name.get_untracked(),
                                    );
                                }
                            },
                        )
                        .disabled(move || state.pairing_busy.get()),
                    )
                    .child(
                        view()
                            .w_full()
                            .flex_row()
                            .gap_2()
                            .child(
                                mobile_button("Edit", Variant::Secondary, move |_| {
                                    let current = state.pairing_uri_input.get_untracked();
                                    batch(move || {
                                        if !current.is_empty() {
                                            manual_draft.set(current);
                                        }
                                        error_for_draft.replace(None);
                                        manual_validation_error.set(String::new());
                                        show_manual.set(true);
                                        state.pairing_uri_input.set(String::new());
                                        state.pairing_probe.set(ProbeState::Idle);
                                        state.pairing_error.set(String::new());
                                    });
                                })
                                .flex_1(),
                            )
                            .child(paste_btn("Paste", false).flex_1())
                            .child(
                                mobile_button("Clear", Variant::Quiet, move |_| {
                                    batch(move || {
                                        error_for_draft.replace(None);
                                        manual_draft.set(String::new());
                                        manual_validation_error.set(String::new());
                                        state.pairing_uri_input.set(String::new());
                                        state.pairing_probe.set(ProbeState::Idle);
                                        state.pairing_error.set(String::new());
                                    });
                                })
                                .flex_1(),
                            ),
                    )
            }),
        )
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
                                "Enter a Tailscale address (host.ts.net or 100.x.y.z), scan a QR code in desktop Settings -> Remote Access, or paste a pairing link.",
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

/// Runs a one-shot on-demand Tailnet discovery via an already connected host (never background-polled).
pub(crate) fn run_tailnet_discovery(
    ui: Ui,
    state: MobileState,
    via_host_id: Option<String>,
    open_if_found: bool,
) {
    let Some(client) = crate::backend::remote_client() else {
        state.discovering_hosts.set(false);
        return;
    };
    if state.discovering_hosts.get_untracked() {
        return;
    }
    batch(move || {
        state.discovering_hosts.set(true);
        state.discover_error.set(String::new());
    });
    ui.spawn(async move {
        let client_call = client.clone();
        let via = via_host_id.clone();
        let res = blocking(move || {
            client_call.block_on(client_call.discover_canban_hosts(via.as_deref()))
        })
        .await;
        let _ = state.discovering_hosts.try_set(false);
        match res {
            Ok(Ok(hosts)) => {
                let has_new = !hosts.is_empty();
                let _ = state.discovered_hosts.try_set(hosts);
                if open_if_found && has_new && state.sheet.try_get().is_some_and(|s| s.is_none()) {
                    state.open_sheet(MobileSheet::DiscoverHosts);
                }
            }
            Ok(Err(e)) => {
                if !open_if_found {
                    let _ = state.discover_error.try_set(format!("{e:#}"));
                }
            }
            Err(e) => {
                if !open_if_found {
                    let _ = state.discover_error.try_set(e.to_string());
                }
            }
        }
    });
}

fn join_discovered_hosts(ui: Ui, state: MobileState, hosts: Vec<DiscoveredCanbanHost>) {
    let ts_hosts: Vec<DiscoveredCanbanHost> =
        hosts.into_iter().filter(|h| h.is_tailscale).collect();
    if ts_hosts.is_empty() {
        return;
    }
    let addrs: Vec<String> = ts_hosts.iter().map(|h| h.addr.clone()).collect();
    state.discover_error.set(String::new());
    state.joining_host_addrs.update(|set| {
        for addr in &addrs {
            set.insert(addr.clone());
        }
    });

    let Some(client) = crate::backend::remote_client() else {
        let count = addrs.len();
        state.joining_host_addrs.update(|set| {
            for addr in &addrs {
                set.remove(addr);
            }
        });
        state
            .discovered_hosts
            .update(|items| items.retain(|h| !addrs.contains(&h.addr)));
        ui.notify(format!("Gallery preview · joined {count} host(s)"));
        if state.discovered_hosts.with_untracked(Vec::is_empty) {
            state.close_sheet();
        }
        return;
    };

    ui.spawn(async move {
        let client_call = client.clone();
        let outcome = blocking(move || {
            let mut joined_addrs = Vec::new();
            let mut first_err: Option<anyhow::Error> = None;
            for host in ts_hosts {
                match client_call.block_on(client_call.pair_tailscale_host(
                    &host.addr,
                    Some(&host.name),
                    host.server_id.as_deref(),
                )) {
                    Ok(_) => joined_addrs.push(host.addr),
                    Err(e) => {
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    }
                }
            }
            let sums = client_call.host_summaries();
            let snap = client_call.merged_snapshot()?;
            anyhow::Ok((joined_addrs, first_err, sums, snap))
        })
        .await;

        if let Some(mut cur_joining) = state.joining_host_addrs.try_get() {
            for addr in &addrs {
                cur_joining.remove(addr);
            }
            let _ = state.joining_host_addrs.try_set(cur_joining);
        }
        match outcome {
            Ok(Ok((joined_addrs, first_err, sums, snap))) => {
                let joined_count = joined_addrs.len();
                let mut remaining_empty = false;
                if let Some(mut items) = state.discovered_hosts.try_get() {
                    items.retain(|h| !joined_addrs.contains(&h.addr));
                    remaining_empty = items.is_empty();
                    let _ = state.discovered_hosts.try_set(items);
                }
                if joined_count > 0 {
                    let _ = ui.hosts.try_set(sums);
                    ui.accept_snapshot(snap);
                    if ui.project.try_get().is_some_and(|p| p.is_empty())
                        && let Some(first_proj) = ui
                            .snapshot
                            .with_untracked(|s| s.projects.first().map(|p| p.id.clone()))
                    {
                        let _ = ui.project.try_set(first_proj);
                    }
                    trigger_haptic(Haptic::Notification(HapticNotification::Success));
                    ui.notify(if joined_count == 1 {
                        "Joined 1 host".to_owned()
                    } else {
                        format!("Joined {joined_count} hosts")
                    });
                    crate::presenter::refresh(ui);
                }
                if let Some(err) = first_err {
                    trigger_haptic(Haptic::Notification(HapticNotification::Error));
                    let _ = state.discover_error.try_set(format!("{err:#}"));
                } else if remaining_empty {
                    state.close_sheet();
                }
            }
            Ok(Err(e)) => {
                trigger_haptic(Haptic::Notification(HapticNotification::Error));
                let _ = state.discover_error.try_set(format!("{e:#}"));
            }
            Err(e) => {
                let _ = state.discover_error.try_set(e.to_string());
            }
        }
    });
}

/// Modal sheet displaying Canban hosts discovered via a connected machine's Tailnet / saved hosts.
pub(crate) fn discover_hosts_sheet_body(ui: Ui, state: MobileState) -> impl IntoElement {
    let model = computed(move || {
        (
            state.discovered_hosts.get(),
            state.discovering_hosts.get(),
            state.discover_error.get(),
            state.joining_host_addrs.get(),
        )
    });

    view().w_full().gap_4().child(dynamic(move || {
        let (hosts, discovering, err, joining) = model.get();
        let mut col = view().w_full().gap_4();

        if !err.is_empty() {
            col = col.child(notice(err, Tone::Danger));
        }

        if discovering && hosts.is_empty() {
            return col
                .child(
                    view()
                        .w_full()
                        .p_4()
                        .rounded(px(MOBILE_RADIUS))
                        .border_1()
                        .border_color(BORDER)
                        .bg(SURFACE)
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(loading_spinner(16., ACCENT))
                        .child(mobile_subhead("Checking online Tailnet machines…")),
                )
                .into_element();
        }

        if hosts.is_empty() {
            let rescan_cta = mobile_inline_button("Scan Again", Variant::Secondary, move |_| {
                run_tailnet_discovery(ui, state, None, false);
            })
            .into_element();
            return col
                .child(empty_state(
                    Icon::Server,
                    Tone::Neutral,
                    "No new Canban hosts found",
                    "Online machines running Canban on the same Tailscale account will appear here.",
                    Some(rescan_cta),
                ))
                .into_element();
        }

        let joinable: Vec<DiscoveredCanbanHost> =
            hosts.iter().filter(|h| h.is_tailscale).cloned().collect();
        let any_joining = !joining.is_empty();

        let rows: Vec<AnyElement> = hosts
            .into_iter()
            .map(|host| {
                let is_busy = joining.contains(&host.addr);
                let subtitle = if host.os.trim().is_empty() {
                    format!("{} · {}ms", host.addr, host.rtt_ms)
                } else {
                    format!("{} · {} · {}ms", host.os, host.addr, host.rtt_ms)
                };
                let (sec_ic, sec_col) = if host.is_tailscale {
                    (Icon::Check, SUCCESS)
                } else {
                    (Icon::TriangleAlert, ATTENTION)
                };
                let action_btn = if host.is_tailscale {
                    let h_one = host.clone();
                    mobile_inline_button(
                        if is_busy { "Joining…" } else { "Join" },
                        Variant::Primary,
                        move |_| join_discovered_hosts(ui, state, vec![h_one.clone()]),
                    )
                    .disabled(move || is_busy || any_joining)
                    .flex_shrink_0()
                    .into_element()
                } else {
                    let prefill = format!("canban://{}#", host.addr);
                    mobile_inline_button("URI…", Variant::Secondary, move |_| {
                        state.open_pairing(Some(prefill.clone()));
                    })
                    .flex_shrink_0()
                    .into_element()
                };

                view()
                    .w_full()
                    .min_h(px(64.))
                    .px_4()
                    .py_2p5()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .flex_1()
                            .min_w(px(0.))
                            .child(icon_badge(Icon::Server, ACCENT, SELECTED))
                            .child(
                                view()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .gap_0p5()
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_1p5()
                                            .child(mobile_title(host.name).truncate())
                                            .child(
                                                icon_tinted(sec_ic, sec_col)
                                                    .w(px(13.))
                                                    .h(px(13.)),
                                            )
                                            .children(
                                                host.linked_on_host
                                                    .then(|| status("Linked", Tone::Info)),
                                            ),
                                    )
                                    .child(
                                        text(subtitle)
                                            .text_size(px(MOBILE_CAPTION))
                                            .font_family(MONO_FONT)
                                            .text_color(MUTED)
                                            .truncate(),
                                    ),
                            ),
                    )
                    .child(action_btn)
                    .into_element()
            })
            .collect();

        col = col
            .child(
                view()
                    .w_full()
                    .child(section_header("AVAILABLE ON TAILNET", None))
                    .child(grouped_card(rows))
                    .child(view().px_2().pt_2().child(mobile_footnote(
                        "Connects directly from this device over Tailscale using your personal account.",
                    ))),
            );

        if joinable.len() > 1 {
            let join_count = joinable.len();
            let all = joinable;
            col = col.child(
                mobile_button(format!("Join All ({join_count})"), Variant::Primary, move |_| {
                    join_discovered_hosts(ui, state, all.clone());
                })
                .disabled(move || any_joining || discovering),
            );
        }

        col.child(
            view()
                .w_full()
                .flex_row()
                .gap_2()
                .child(
                    mobile_button(
                        if discovering { "Scanning…" } else { "Rescan" },
                        Variant::Secondary,
                        move |_| run_tailnet_discovery(ui, state, None, false),
                    )
                    .disabled(move || discovering || any_joining)
                    .flex_1(),
                )
                .child(
                    mobile_button("Done", Variant::Quiet, move |_| state.close_sheet()).flex_1(),
                ),
        )
        .into_element()
    }))
}
