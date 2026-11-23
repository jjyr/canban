//! Platform-adaptive mobile chrome helpers, unified iOS + Android typography/spacing tokens,
//! Inset Grouped surfaces, native `UITabBar` / `NavigationBar` / `PageSheet` / `Switch` / `AlertDialog`,
//! and haptic helpers.
use super::{MobileSheet, MobileState, mobile_hosts};
use crate::presenter::Ui;
use canban_client::TransportSecurity;
use canban_types::HostSummary;
use canban_ui_kit::{tokens::*, *};
use nsh_base::{alert_dialog, dialog};
use nsh_core::{
    Haptic,
    mobile::{SegmentedControlItem, TabBarItem, segmented_control, switch, tab_bar},
    prelude::*,
};
use std::rc::Rc;

/// Mobile HIG / Material 3 typography & layout tokens.
pub(crate) const MOBILE_HERO_TITLE: f32 = 28.0;
pub(crate) const MOBILE_HEADLINE: f32 = 20.0;
pub(crate) const MOBILE_TITLE: f32 = 17.0;
pub(crate) const MOBILE_BODY: f32 = 16.0;
pub(crate) const MOBILE_SUBHEAD: f32 = 14.5;
pub(crate) const MOBILE_FOOTNOTE: f32 = 13.0;
pub(crate) const MOBILE_SECTION: f32 = 12.5;
pub(crate) const MOBILE_CAPTION: f32 = 12.0;
pub(crate) const MOBILE_CODE: f32 = 12.5;

pub(crate) const MOBILE_RADIUS: f32 = 14.0;
pub(crate) const MOBILE_CARD_RADIUS: f32 = 16.0;
pub(crate) const MOBILE_BTN_H: f32 = 48.0;
pub(crate) const MOBILE_INLINE_BTN_H: f32 = 42.0;

/// Returns `true` when compiled for iOS.
pub(crate) fn is_ios() -> bool {
    cfg!(target_os = "ios")
}

/// Returns `true` when compiled for Android.
pub(crate) fn is_android() -> bool {
    cfg!(target_os = "android")
}

/// Returns `true` on native mobile OS targets (iOS or Android).
pub(crate) fn is_mobile_os() -> bool {
    is_ios() || is_android()
}

/// Triggers a native mobile haptic pulse when running on iOS or Android.
pub(crate) fn trigger_haptic(kind: Haptic) {
    if is_mobile_os() {
        haptic(kind);
    }
}

/// Bottom scroll padding on root tabs (leaves room for iOS's floating Liquid Glass `UITabBar`).
pub(crate) fn tab_scroll_bottom_pad() -> f32 {
    if is_ios() { 112.0 } else { 28.0 }
}

fn modal_scrim() -> AdaptiveColor {
    let color = Hsla::from(rgba(0x0000_0099));
    AdaptiveColor {
        light: color,
        dark: color,
    }
}

/// Primary list/card title (`17pt` Semibold, crisp tight tracking).
pub(crate) fn mobile_title(value: impl IntoReactive<SharedString>) -> Text {
    text(value)
        .text_size(px(MOBILE_TITLE))
        .line_height(px(22.))
        .letter_spacing(px(-0.15))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(TEXT)
}

/// Readable mobile body copy (`16pt` Regular, `23pt` line height).
pub(crate) fn mobile_body(value: impl IntoReactive<SharedString>) -> Text {
    text(value)
        .text_size(px(MOBILE_BODY))
        .line_height(px(23.))
        .text_color(TEXT)
}

/// Secondary explanatory copy (`14.5pt` Muted, `20pt` line height).
pub(crate) fn mobile_subhead(value: impl IntoReactive<SharedString>) -> Text {
    text(value)
        .text_size(px(MOBILE_SUBHEAD))
        .line_height(px(20.))
        .text_color(MUTED)
}

/// Metadata/footnote caption (`13pt` Muted, `18pt` line height).
pub(crate) fn mobile_footnote(value: impl IntoReactive<SharedString>) -> Text {
    text(value)
        .text_size(px(MOBILE_FOOTNOTE))
        .line_height(px(18.))
        .text_color(MUTED)
}

/// Tracked uppercase Inset Grouped section header with optional right action.
pub(crate) fn section_header(title: impl Into<SharedString>, trailing: Option<AnyElement>) -> View {
    view()
        .w_full()
        .px_1()
        .pb_1p5()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            text(title.into())
                .text_size(px(MOBILE_SECTION))
                .font_weight(FontWeight::SEMIBOLD)
                .letter_spacing(px(0.6))
                .text_color(MUTED),
        )
        .children(trailing)
}

/// Inset Grouped container wrapping stacked rows with `16pt` inset hairline dividers.
pub(crate) fn grouped_card(rows: Vec<AnyElement>) -> View {
    grouped_card_with_inset(rows, 16.0)
}

/// Inset Grouped container with custom leading divider inset (e.g. `60pt` after a `32pt` icon badge).
pub(crate) fn grouped_card_with_inset(rows: Vec<AnyElement>, divider_inset: f32) -> View {
    let mut card = view()
        .w_full()
        .rounded(px(MOBILE_RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .overflow_hidden();
    for (idx, row) in rows.into_iter().enumerate() {
        if idx > 0 {
            card = card.child(
                view()
                    .w_full()
                    .pl(px(divider_inset))
                    .child(view().w_full().h(px(1.)).bg(BORDER)),
            );
        }
        card = card.child(row);
    }
    card
}

/// `32×32` rounded tinted icon medallion matching iOS Settings / Material 3 leading icons.
pub(crate) fn icon_badge(ic: Icon, fg: ThemeColor, bg: ThemeColor) -> View {
    view()
        .flex_shrink_0()
        .w(px(32.))
        .h(px(32.))
        .rounded(px(8.))
        .bg(bg)
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(icon_tinted(ic, fg).w(px(17.)).h(px(17.)))
}

/// Standard trailing disclosure chevron (`›`).
pub(crate) fn disclosure_chevron() -> View {
    view()
        .flex_shrink_0()
        .pointer_events(PointerEvents::None)
        .child(icon_tinted(Icon::Forward, MUTED).w(px(15.)).h(px(15.)))
}

fn button_colors(variant: Variant) -> (ThemeColor, ThemeColor, ThemeColor) {
    match variant {
        Variant::Primary => (ACCENT_FILL, CLEAR, ON_ACCENT),
        Variant::Secondary => (INSET, BORDER, TEXT),
        Variant::Danger => (DANGER_SURFACE, DANGER, DANGER),
        Variant::Quiet => (CLEAR, CLEAR, ACCENT),
    }
}

/// Full-width thumb-friendly button (`48pt` height, `16pt` Semibold, haptic feedback).
pub(crate) fn mobile_button(
    label_text: impl IntoReactive<SharedString>,
    variant: Variant,
    on_press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    let (bg, border, fg) = button_colors(variant);
    press_target()
        .app_class(canban_ui_kit::motion::CONTROL_TRANSITION)
        .w_full()
        .h(px(MOBILE_BTN_H))
        .px_4()
        .rounded(px(12.))
        .bg(bg)
        .border_1()
        .border_color(border)
        .justify_center()
        .items_center()
        .active(|s| s.opacity(0.75))
        .focus(|s| s.border_color(ACCENT))
        .disabled_style(|s| s.opacity(0.42))
        .child(
            text(label_text)
                .text_size(px(MOBILE_BODY))
                .letter_spacing(px(-0.1))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg),
        )
        .on_press(move |ev: &()| {
            trigger_haptic(Haptic::Selection);
            on_press(ev);
        })
}

/// Inline card/bar action button (`42pt` height, `14.5pt` Semibold).
pub(crate) fn mobile_inline_button(
    label_text: impl IntoReactive<SharedString>,
    variant: Variant,
    on_press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    let (bg, border, fg) = button_colors(variant);
    press_target()
        .app_class(canban_ui_kit::motion::CONTROL_TRANSITION)
        .h(px(MOBILE_INLINE_BTN_H))
        .px_3p5()
        .rounded(px(11.))
        .bg(bg)
        .border_1()
        .border_color(border)
        .justify_center()
        .items_center()
        .active(|s| s.opacity(0.75))
        .focus(|s| s.border_color(ACCENT))
        .disabled_style(|s| s.opacity(0.42))
        .child(
            text(label_text)
                .text_size(px(MOBILE_SUBHEAD))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg),
        )
        .on_press(move |ev: &()| {
            trigger_haptic(Haptic::Selection);
            on_press(ev);
        })
}

/// Thumb-friendly `48pt` mobile text input (`16pt` text).
pub(crate) fn mobile_field(value: Signal<String>, placeholder: &str) -> nsh_core::element::Input {
    nsh_base::input()
        .value(move || value.get())
        .on_change(move |v: &str| value.set(v.to_owned()))
        .placeholder(placeholder.to_owned())
        .min_h(px(48.))
        .px_3p5()
        .py_2p5()
        .rounded(px(12.))
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .text_size(px(MOBILE_BODY))
        .text_color(TEXT)
        .focus(|s| s.border_color(ACCENT))
}

/// Centered `ContentUnavailableView`-style empty state for mobile screens.
pub(crate) fn empty_state(
    ic: Icon,
    tone: Tone,
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    cta: Option<AnyElement>,
) -> View {
    let (bg, fg) = tone_colors(tone);
    view()
        .w_full()
        .py(px(56.))
        .px_6()
        .items_center()
        .gap_3()
        .child(
            view()
                .w(px(56.))
                .h(px(56.))
                .rounded(px(16.))
                .bg(bg)
                .items_center()
                .justify_center()
                .child(icon_tinted(ic, fg).w(px(26.)).h(px(26.))),
        )
        .child(
            text(title.into())
                .text_size(px(MOBILE_HEADLINE))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(TEXT)
                .text_center(),
        )
        .child(
            view().w_full().max_w(px(300.)).items_center().child(
                mobile_subhead(subtitle.into())
                    .line_height(px(21.))
                    .text_center(),
            ),
        )
        .children(cta.map(|btn| view().pt_1().w_full().max_w(px(260.)).child(btn)))
}

/// Native confirmation dialog (`UIAlertController` on iOS, Material `AlertDialog` on Android,
/// `NSAlert` on macOS) for destructive actions such as removing a paired host.
pub(crate) fn confirm_destructive(
    ui: Ui,
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
    confirm_label: impl Into<SharedString>,
    on_confirm: impl Fn() + 'static,
) {
    let answer = alert_dialog()
        .title(title)
        .description(message)
        .cancel("cancel", "Cancel")
        .destructive("confirm", confirm_label)
        .show();
    ui.spawn(async move {
        if answer
            .await
            .is_ok_and(|res| res.action.as_deref() == Some("confirm"))
        {
            on_confirm();
        }
    });
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

/// Rounded inset offline/sync alert card shown inside mobile scroll content when relevant.
/// Renders `view().hidden()` (`display: none`) when idle so flex `gap` reserves zero height.
pub(crate) fn offline_sync_banner(
    ui: Ui,
    state: MobileState,
    scope_host_id: Option<String>,
) -> impl IntoElement {
    offline_sync_banner_inner(ui, state, scope_host_id, false)
}

/// Edge-inset variant (`px_4 pt_2`) for containers without horizontal padding (such as `Board`).
pub(crate) fn offline_sync_banner_inset(
    ui: Ui,
    state: MobileState,
    scope_host_id: Option<String>,
) -> impl IntoElement {
    offline_sync_banner_inner(ui, state, scope_host_id, true)
}

fn offline_sync_banner_inner(
    ui: Ui,
    state: MobileState,
    scope_host_id: Option<String>,
    with_outer_padding: bool,
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
                format!(" · {pending_total} change(s) queued")
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
            return view().hidden();
        }
        let tone = match m.kind {
            2 | 1 => Tone::Attention,
            _ => Tone::Info,
        };
        let (bg, fg) = tone_colors(tone);
        let border_col = match tone {
            Tone::Attention => ATTENTION,
            _ => ACCENT,
        };
        let open_sync = m.open_sync;
        let host_id = m.host_id;
        let card = view()
            .w_full()
            .rounded(px(MOBILE_RADIUS))
            .px_3p5()
            .py_2p5()
            .border_1()
            .border_color(border_col)
            .bg(bg)
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2p5()
            .child(
                text(m.text)
                    .text_size(px(13.5))
                    .line_height(px(18.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg)
                    .flex_1()
                    .min_w(px(0.)),
            )
            .child(
                press_target()
                    .h(px(34.))
                    .px_3()
                    .rounded(px(9.))
                    .bg(SURFACE)
                    .border_1()
                    .border_color(BORDER)
                    .items_center()
                    .justify_center()
                    .active(|s| s.opacity(0.75))
                    .child(
                        text(m.cta)
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(TEXT),
                    )
                    .on_press(move |_: &()| {
                        trigger_haptic(Haptic::Selection);
                        if open_sync {
                            state.open_sheet(MobileSheet::SyncConflicts);
                        } else {
                            crate::hosts::reconnect_host(ui, host_id.clone());
                        }
                    }),
            );

        if with_outer_padding {
            view().w_full().flex_shrink_0().px_4().pt_2().child(card)
        } else {
            card
        }
    })
}

/// Platform-adaptive 4-tab bottom bar:
/// - iOS: permanently mounted floating Liquid Glass `UITabBar` so `hides_bottom_bar_when_pushed`
///   animates smoothly alongside swipe-back transitions
/// - Android: Material 3 bottom `TabBar`
/// - Desktop Gallery: phone-frame styled tab bar.
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
                TabBarItem::new("Board").icon("rectangle.3.group.fill"),
                TabBarItem::new("Terminals").icon("terminal.fill"),
                TabBarItem::new("Settings").icon("gearshape.fill"),
            ]
        };
        return view()
            .w_full()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .child(
                tab_bar()
                    .items(items)
                    .selected_index(move || tab.get().index())
                    .on_change(move |idx: &u32| {
                        if let Some(next) = super::MobileTab::from_index(*idx) {
                            trigger_haptic(Haptic::Selection);
                            on_select_ios(next);
                        }
                    }),
            );
    }

    if is_android() {
        let on_select_android = on_select;
        let items = move || {
            let count = inbox_badge();
            let badge_val = (count > 0).then(|| count.to_string());
            let mut needs = TabBarItem::new("Needs you");
            if let Some(b) = badge_val {
                needs = needs.badge(b);
            }
            vec![
                needs,
                TabBarItem::new("Board"),
                TabBarItem::new("Terminals"),
                TabBarItem::new("Settings"),
            ]
        };
        return view().child(when(
            move || !hidden(),
            move || {
                let on_sel = on_select_android.clone();
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
                .h(px(58.))
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
                                            .w(px(19.))
                                            .h(px(19.))
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
                                                            .text_color(ON_ACCENT)
                                                            .tabular_nums(),
                                                    )
                                            },
                                        )
                                    })),
                            )
                            .child(
                                text(title)
                                    .text_size(px(11.))
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

/// Platform-adaptive segmented control (`UISegmentedControl` on iOS, tactile pill bar elsewhere).
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

    view()
        .w_full()
        .h(px(40.))
        .p_1()
        .rounded(px(11.))
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .flex_row()
        .items_center()
        .gap_1()
        .children(titles.iter().enumerate().map(move |(idx, title)| {
            let active = move || selected_index() == idx;
            press_target()
                .flex_1()
                .h_full()
                .rounded(px(8.))
                .items_center()
                .justify_center()
                .bg(move || {
                    if active() {
                        SURFACE.resolve()
                    } else {
                        CLEAR.resolve()
                    }
                })
                .active(|s| s.opacity(0.75))
                .child(
                    text(*title)
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || {
                            if active() {
                                TEXT.resolve()
                            } else {
                                MUTED.resolve()
                            }
                        }),
                )
                .on_press(move |_: &()| {
                    trigger_haptic(Haptic::Selection);
                    on_select(idx);
                })
        }))
}

/// Platform-adaptive native switch (`UISwitch` on iOS, Material 3 Switch on Android, `toggle` in Gallery).
pub(crate) fn mobile_switch(
    checked: impl Fn() -> bool + Copy + 'static,
    a11y_label: &'static str,
    on_change: impl Fn(bool) + Clone + 'static,
) -> View {
    if is_mobile_os() {
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

/// Native mobile `PageSheet` (`UISheetPresentationController` on iOS, ModalBottomSheet on Android)
/// or contained bottom-sheet overlay inside Desktop Gallery phone frames.
pub(crate) fn mobile_sheet(
    title: String,
    medium_default: bool,
    on_close: Rc<dyn Fn()>,
    body: impl IntoElement + 'static,
) -> View {
    let close_btn = on_close.clone();
    let header_bar = view()
        .w_full()
        .flex_shrink_0()
        .relative()
        .min_h(px(56.))
        .px_4()
        .pt_3()
        .pb_2()
        .border_b_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(view().w(px(56.)).flex_shrink_0())
        .child(
            text(title)
                .text_size(px(MOBILE_TITLE))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(TEXT)
                .text_center()
                .truncate()
                .flex_1()
                .min_w(px(0.)),
        )
        .child(
            press_target()
                .min_w(px(56.))
                .min_h(px(44.))
                .px_2()
                .items_end()
                .justify_center()
                .active(|s| s.opacity(0.65))
                .child(
                    text("Done")
                        .text_size(px(MOBILE_BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ACCENT),
                )
                .on_press(move |_: &()| {
                    trigger_haptic(Haptic::Selection);
                    close_btn();
                }),
        );

    let panel_body = view()
        .w_full()
        .flex_1()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(header_bar)
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .child(view().w_full().p_4().gap_4().pb(px(40.)).child(body)),
        );

    if is_mobile_os() {
        let on_cancel_cb = on_close.clone();
        let on_close_cb = on_close;
        return view().child(
            dialog()
                .presentation(ModalPresentation::PageSheet)
                .detents(vec![SheetDetent::Medium, SheetDetent::Large])
                .default_detent(Some(if medium_default {
                    SheetDetent::Medium
                } else {
                    SheetDetent::Large
                }))
                .grabber(true)
                .close_on_swipe(true)
                .close_on_backdrop_press(true)
                .close_on_escape(true)
                .on_cancel(move |_: &()| on_cancel_cb())
                .on_close(move |_: &()| on_close_cb())
                .child(
                    view()
                        .size_full()
                        .flex_col()
                        .min_h(px(0.))
                        .bg(BACKGROUND)
                        .child(panel_body),
                ),
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
                .max_h(px(680.))
                .min_h(px(if medium_default { 380. } else { 480. }))
                .rounded_t(px(18.))
                .border_t_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .overflow_hidden()
                .on_press(|_: &PressEvent| {})
                .child(
                    view()
                        .w(px(36.))
                        .h(px(5.))
                        .rounded_full()
                        .bg(MUTED)
                        .opacity(0.45)
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
