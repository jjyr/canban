//! Platform-adaptive mobile chrome helpers (`TabBar`, `SegmentedControl`, `Switch`, `Sheet`),
//! transport security badges, offline/sync banners, and iOS haptics.
use super::{MobileSheet, MobileState, mobile_hosts};
use crate::presenter::Ui;
use canban_client::TransportSecurity;
use canban_types::HostSummary;
use canban_ui_kit::{tokens::*, *};
use nsh_base::{SheetDetent, SheetEvent, sheet};
use nsh_core::{
    Haptic,
    mobile::{SegmentedControlItem, TabBarItem, segmented_control, switch, tab_bar},
    platform::Os,
    prelude::*,
};
use std::rc::Rc;

/// Triggers a native iOS haptic pulse when running on iOS.
pub(crate) fn trigger_haptic(kind: Haptic) {
    if try_os() == Some(Os::Ios) {
        haptic(kind);
    }
}

/// Returns `true` when running natively on iOS (`Os::Ios`).
pub(crate) fn is_ios() -> bool {
    try_os() == Some(Os::Ios)
}

fn modal_scrim() -> AdaptiveColor {
    let color = Hsla::from(rgba(0x0000_0099));
    AdaptiveColor {
        light: color,
        dark: color,
    }
}

/// Renders a confidential/warning pill badge for a host or pairing link's [`TransportSecurity`].
pub(crate) fn security_badge(sec: TransportSecurity) -> View {
    let (prefix, label_text, tone) = match sec {
        TransportSecurity::Local => ("🔒", "Local", Tone::Success),
        TransportSecurity::Tailscale => ("🔒", "Tailscale", Tone::Success),
        TransportSecurity::Ssh => ("🔒", "SSH", Tone::Success),
        TransportSecurity::UnverifiedTcp => ("⚠", "Not Tailscale", Tone::Attention),
    };
    status(format!("{prefix} {label_text}"), tone)
}

/// Compact offline/sync banner showing cached/unreachable hosts, pending mutations, and conflicts.
pub(crate) fn offline_sync_banner(
    ui: Ui,
    state: MobileState,
    scope_host_id: Option<String>,
) -> impl IntoElement {
    #[derive(Clone, PartialEq, Eq)]
    struct BannerModel {
        kind: u8, // 0 = hidden, 1 = offline, 2 = conflict, 3 = pending
        text: String,
        cta: &'static str,
        open_sync: bool,
        host_id: Option<String>,
    }

    let model = computed(move || {
        let hosts = mobile_hosts(ui);
        let scoped: Vec<HostSummary> = match &scope_host_id {
            Some(hid) => hosts.into_iter().filter(|h| &h.id == hid).collect(),
            None => hosts,
        };
        let offline_names: Vec<String> = scoped
            .iter()
            .filter(|h| !h.is_connected())
            .map(|h| h.name.clone())
            .collect();
        let pending_total: usize = scoped.iter().map(|h| h.pending_mutations).sum();
        let conflict_total: usize = scoped.iter().map(|h| h.sync_conflicts).sum();

        if conflict_total > 0 {
            BannerModel {
                kind: 2,
                text: format!(
                    "{conflict_total} sync issue{} need attention",
                    if conflict_total == 1 { "" } else { "s" }
                ),
                cta: "Resolve",
                open_sync: true,
                host_id: None,
            }
        } else if !offline_names.is_empty() {
            let names = offline_names.join(", ");
            let suffix = if pending_total > 0 {
                format!(" · {pending_total} offline change(s) queued")
            } else {
                " · showing cached data".to_owned()
            };
            BannerModel {
                kind: 1,
                text: format!("Offline ({names}){suffix}"),
                cta: "Retry",
                open_sync: false,
                host_id: (offline_names.len() == 1).then(|| scoped[0].id.clone()),
            }
        } else if pending_total > 0 {
            BannerModel {
                kind: 3,
                text: format!("Syncing {pending_total} pending change(s)…"),
                cta: "Queue",
                open_sync: true,
                host_id: None,
            }
        } else {
            BannerModel {
                kind: 0,
                text: String::new(),
                cta: "",
                open_sync: false,
                host_id: None,
            }
        }
    });

    dynamic(move || {
        let m = model.get();
        if m.kind == 0 {
            return view();
        }
        let tone = match m.kind {
            2 | 1 => Tone::Attention,
            _ => Tone::Info,
        };
        let (bg, fg) = tone_colors(tone);
        let open_sync = m.open_sync;
        let host_id = m.host_id;
        view()
            .w_full()
            .flex_shrink_0()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(BORDER)
            .bg(bg)
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                label(m.text)
                    .text_size(px(12.))
                    .text_color(fg)
                    .flex_1()
                    .min_w(px(0.)),
            )
            .child(compact_action(m.cta, Variant::Secondary, move |_| {
                trigger_haptic(Haptic::Selection);
                if open_sync {
                    state.open_sheet(MobileSheet::SyncConflicts);
                } else {
                    crate::hosts::reconnect_host(ui, host_id.clone());
                }
            }))
    })
}

/// Platform-adaptive 4-tab bottom bar (`UITabBar` on iOS, styled tab bar in desktop Gallery preview).
pub(crate) fn mobile_tab_bar(
    tab: Signal<super::MobileTab>,
    inbox_badge: impl Fn() -> usize + Copy + 'static,
    hidden: impl Fn() -> bool + Copy + 'static,
    on_select: impl Fn(super::MobileTab) + Clone + 'static,
) -> View {
    if is_ios() {
        let on_select_ios = on_select;
        let items = move || {
            let count = inbox_badge();
            let badge_val = (count > 0).then(|| count.to_string());
            let mut needs = TabBarItem::new("Needs you").icon("tray.fill");
            if let Some(b) = badge_val {
                needs = needs.badge(b);
            }
            vec![
                needs,
                TabBarItem::new("Board").icon("rectangle.3.group"),
                TabBarItem::new("Terminals").icon("terminal"),
                TabBarItem::new("Settings").icon("gearshape"),
            ]
        };
        return view().child(when(
            move || !hidden(),
            move || {
                let on_sel = on_select_ios.clone();
                view().w_full().flex_shrink_0().child(
                    tab_bar()
                        .items(items)
                        .selected_index(move || tab.get().index())
                        .hidden(hidden)
                        .on_change(move |idx: &u32| {
                            if let Some(next) = super::MobileTab::from_index(*idx) {
                                trigger_haptic(Haptic::Selection);
                                on_sel(next);
                            }
                        }),
                )
            },
        ));
    }

    view().child(when(
        move || !hidden(),
        move || {
            let tabs = [
                (super::MobileTab::NeedsYou, "Needs you", Icon::Overview),
                (super::MobileTab::Board, "Board", Icon::Board),
                (super::MobileTab::Terminals, "Terminals", Icon::Terminal),
                (super::MobileTab::Settings, "Settings", Icon::Settings),
            ];
            view()
                .w_full()
                .h(px(56.))
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .justify_around()
                .border_t_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .children(tabs.into_iter().map({
                    let value = on_select.clone();
                    move |(t, title, ic)| {
                        let on_sel = value.clone();
                        let is_badge_tab = t == super::MobileTab::NeedsYou;
                        press_target()
                            .flex_1()
                            .h_full()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_0p5()
                            .cursor_pointer()
                            .accessibility_label(Some(SharedString::from(title)))
                            .child(
                                view()
                                    .relative()
                                    .flex_row()
                                    .items_center()
                                    .justify_center()
                                    .child(dynamic(move || {
                                        let active = tab.get() == t;
                                        icon_tinted(ic, if active { ACCENT } else { MUTED })
                                            .w(px(18.))
                                            .h(px(18.))
                                    }))
                                    .children(is_badge_tab.then(|| {
                                        when(
                                            move || inbox_badge() > 0,
                                            move || {
                                                view()
                                                    .ml_1()
                                                    .px_1p5()
                                                    .rounded_full()
                                                    .bg(ACCENT)
                                                    .child(
                                                        text(move || inbox_badge().to_string())
                                                            .text_size(px(10.))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(ON_ACCENT),
                                                    )
                                            },
                                        )
                                    })),
                            )
                            .child(
                                text(title)
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(
                                        move || if tab.get() == t { ACCENT } else { MUTED },
                                    ),
                            )
                            .on_press(move |_: &()| on_sel(t))
                    }
                }))
        },
    ))
}

/// Platform-adaptive segmented control (`UISegmentedControl` on iOS, `canban_ui_kit::segmented` on desktop).
pub(crate) fn mobile_segmented_control(
    titles: &'static [&'static str],
    selected_index: impl Fn() -> usize + Copy + 'static,
    on_select: impl Fn(usize) + Copy + 'static,
) -> View {
    if is_ios() {
        let items: Vec<SegmentedControlItem> = titles
            .iter()
            .map(|t| SegmentedControlItem::new(*t))
            .collect();
        return view().w_full().child(
            segmented_control()
                .items(items)
                .selected_index(move || selected_index() as u32)
                .on_change(move |idx: &u32| {
                    trigger_haptic(Haptic::Selection);
                    on_select(*idx as usize);
                }),
        );
    }

    segmented().w_full().children(
        titles
            .iter()
            .enumerate()
            .map(move |(idx, title)| {
                let is_sel = move || selected_index() == idx;
                compact_segment(*title, None, is_sel, move |_| on_select(idx)).flex_1()
            })
            .collect::<Vec<_>>(),
    )
}

/// Platform-adaptive native switch (`UISwitch` on iOS, `canban_ui_kit::toggle` on desktop).
pub(crate) fn mobile_switch(
    checked: impl Fn() -> bool + Copy + 'static,
    a11y_label: &'static str,
    on_change: impl Fn(bool) + Clone + 'static,
) -> View {
    if is_ios() {
        return view().child(
            switch()
                .checked(checked)
                .accessibility_label(Some(SharedString::from(a11y_label)))
                .on_change(move |next: &bool| {
                    trigger_haptic(Haptic::Selection);
                    on_change(*next);
                }),
        );
    }

    view().child(toggle("", checked, on_change))
}

/// Native iOS `nsh_base::sheet()` or contained bottom-sheet overlay inside phone-sized desktop frames.
pub(crate) fn mobile_sheet(
    title: String,
    on_close: Rc<dyn Fn()>,
    body: impl IntoElement + 'static,
) -> View {
    let close_btn = on_close.clone();
    let header_bar = view()
        .w_full()
        .px_4()
        .py_3()
        .border_b_1()
        .border_color(BORDER)
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            label(title)
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .min_w(px(0.)),
        )
        .child(compact_action("Done", Variant::Quiet, move |_| close_btn()));

    let panel_body = view()
        .w_full()
        .flex_1()
        .min_h(px(0.))
        .bg(SURFACE)
        .child(header_bar)
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .child(view().w_full().p_4().gap_3().pb(px(32.)).child(body)),
        );

    if is_ios() {
        let on_dismiss = on_close;
        return view().child(
            sheet()
                .open(true)
                .grabber(true)
                .close_on_swipe(true)
                .close_on_backdrop_press(true)
                .detents(vec![SheetDetent::Medium, SheetDetent::Large])
                .default_detent(Some(SheetDetent::Large))
                .popup_style(|s| {
                    s.bg(SURFACE)
                        .border_t_1()
                        .border_color(BORDER)
                        .rounded_t(px(16.))
                        .overflow_hidden()
                })
                .backdrop_style(|s| s.bg(modal_scrim()))
                .grabber_style(|s| {
                    s.w(px(36.))
                        .h(px(4.))
                        .rounded_full()
                        .bg(MUTED)
                        .my_2()
                        .self_center()
                })
                .on_open_change(move |ev: &SheetEvent| {
                    if matches!(ev, SheetEvent::OpenChange(false, _)) {
                        on_dismiss();
                    }
                })
                .child(panel_body),
        );
    }

    let on_backdrop = on_close;
    view()
        .absolute()
        .inset_0()
        .justify_end()
        .bg(modal_scrim())
        .on_press(move |_: &PressEvent| on_backdrop())
        .child(
            view()
                .w_full()
                .max_h(px(660.))
                .min_h(px(280.))
                .rounded_t(px(16.))
                .border_t_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .overflow_hidden()
                .on_press(|_: &PressEvent| {})
                .child(
                    view()
                        .w(px(36.))
                        .h(px(4.))
                        .rounded_full()
                        .bg(MUTED)
                        .my_2()
                        .self_center(),
                )
                .child(panel_body),
        )
}

/// Concise host connection badge for Host rows & pickers.
pub(crate) fn host_connection_status(summary: &HostSummary) -> (String, Tone) {
    if summary.is_local {
        return ("Connected".into(), Tone::Success);
    }
    let label = summary.connection.short_status();
    let tone = match &summary.connection {
        canban_types::HostConnectionState::Connected { .. } => Tone::Success,
        canban_types::HostConnectionState::Connecting
        | canban_types::HostConnectionState::Updating { .. }
        | canban_types::HostConnectionState::Reconnecting { .. } => Tone::Info,
        canban_types::HostConnectionState::AuthFailed { .. } => Tone::Danger,
        canban_types::HostConnectionState::ApiMismatch { .. }
        | canban_types::HostConnectionState::Offline { .. } => Tone::Attention,
    };
    (label, tone)
}
