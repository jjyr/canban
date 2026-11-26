//! Compact window-chrome controls. NSH owns keyboard, focus and native tooltips.
use super::{Icon, hint_tooltip, icon, tokens::*};
use nsh_core::prelude::*;
pub fn chrome_button(kind: Icon, title: impl Into<String>, press: impl Fn(&()) + 'static) -> View {
    chrome_button_disabled(kind, title, || false, press)
}
pub fn chrome_button_disabled(
    kind: Icon,
    title: impl Into<String>,
    disabled: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> View {
    let title = title.into();
    let hint = title.clone();
    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(hint_tooltip(hint).placement(Placement::Bottom))
        .child(chrome_action(kind, title, press).disabled(disabled))
}
/// Compact accessible icon action without a tooltip, suitable as a popup trigger.
pub fn chrome_action(
    kind: Icon,
    title: impl Into<String>,
    press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    let title = title.into();
    crate::press_target()
        .app_class(super::motion::CONTROL_TRANSITION)
        .disabled_style(|s| s.opacity(0.4))
        .w(px(30.))
        .h(px(30.))
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .bg(CLEAR)
        .hover(|s| s.bg(INSET))
        .focus(|s| s.border_1().border_color(ACCENT))
        .accessibility_label(move || Some(title.clone().into()))
        .child(icon(kind).w(px(16.)).h(px(16.)))
        .on_press(press)
}

/// Compact window-chrome icon action with an optional corner indicator dot (e.g. connected repository).
pub fn chrome_indicator_button(
    kind: Icon,
    title: impl Fn() -> String + Copy + 'static,
    active: impl Fn() -> bool + Copy + 'static,
    press: impl Fn(&()) + 'static,
) -> View {
    let active = computed(active);
    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(hint_tooltip(move || SharedString::from(title())).placement(Placement::Bottom))
        .child(
            crate::press_target()
                .app_class(super::motion::CONTROL_TRANSITION)
                .relative()
                .w(px(30.))
                .h(px(30.))
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(CLEAR)
                .hover(|s| s.bg(INSET))
                .focus(|s| s.border_1().border_color(ACCENT))
                .accessibility_label(move || Some(title().into()))
                .child(
                    super::icon_tinted(kind, move || if active.get() { TEXT } else { MUTED })
                        .w(px(16.))
                        .h(px(16.)),
                )
                .child(when(
                    move || active.get(),
                    move || {
                        view()
                            .absolute()
                            .right(px(5.))
                            .bottom(px(5.))
                            .w(px(6.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(SUCCESS)
                            .pointer_events(PointerEvents::None)
                    },
                ))
                .on_press(press),
        )
}

/// Selected utility action, shared by the compact global tools.
pub fn utility_button(
    kind: Icon,
    title: impl Into<String>,
    selected: impl Fn() -> bool + 'static,
    disabled: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> View {
    let title = title.into();
    let hint = title.clone();
    let selected = computed(selected);
    view()
        .flex_shrink_0()
        .tooltip(hint_tooltip(hint).placement(Placement::Top))
        .child(
            crate::press_target()
                .w(px(30.))
                .h(px(28.))
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(move || if selected.get() { SELECTED } else { CLEAR })
                .hover(|s| s.bg(INSET))
                .focus(|s| s.border_1().border_color(ACCENT))
                .selected(move || selected.get())
                .disabled(disabled)
                .disabled_style(|s| s.opacity(0.4))
                .accessibility_label(move || Some(title.clone().into()))
                .child(
                    super::icon_tinted(kind, move || if selected.get() { ACCENT } else { MUTED })
                        .w(px(16.))
                        .h(px(16.)),
                )
                .on_press(press),
        )
}

/// Compact sync action with native animation and reduced-motion support.
pub fn sync_action(
    kind: Icon,
    title: String,
    busy: bool,
    spinning: bool,
    press: impl Fn(&()) + 'static,
) -> View {
    use nsh_core::element::Animation;
    let angle = animated_value(0.);
    if spinning && !super::motion::reduced() {
        let animation =
            Animation::timing(&angle, 360., Timing::new(1000., Easing::Linear)).looped(None);
        let _ = animation.start();
        cleanup(move || animation.stop());
    }
    view()
        .window_drag(false)
        .flex_shrink_0()
        .tooltip(hint_tooltip(title.clone()).placement(Placement::Bottom))
        .child(
            super::press_target()
                .w(px(30.))
                .h(px(30.))
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(CLEAR)
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.border_1().border_color(ACCENT))
                .accessibility_label(move || Some(title.clone().into()))
                .disabled(move || busy)
                .disabled_style(move |s| s.opacity(if spinning { 1.0 } else { 0.45 }))
                .child(
                    view()
                        .w(px(16.))
                        .h(px(16.))
                        .bind_rotate(&angle)
                        .child(icon(kind).w(px(16.)).h(px(16.))),
                )
                .on_press(press),
        )
}

/// Compact icon-only copy button with tooltip, pointer cursor, and brief check confirmation.
/// Keeps confirmation visible inline even when a floating error banner suppresses the global toast.
pub fn copy_icon_button(
    title: impl Into<String>,
    tooltip_text: impl Into<String>,
    size: f32,
    icon_size: f32,
    hover_bg: ThemeColor,
    placement: Placement,
    on_copy: impl Fn() + 'static,
) -> View {
    let title = title.into();
    let idle_tip = tooltip_text.into();
    let copied = signal(false);
    let epoch = signal(0_u64);
    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(
            hint_tooltip(move || -> SharedString {
                if copied.get() {
                    "Copied".into()
                } else {
                    idle_tip.clone().into()
                }
            })
            .placement(placement),
        )
        .child(
            crate::press_target()
                .app_class(super::motion::CONTROL_TRANSITION)
                .w(px(size))
                .h(px(size))
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(CLEAR)
                .hover(move |s| s.bg(hover_bg))
                .focus(|s| s.border_1().border_color(ACCENT))
                .accessibility_label(move || Some(title.clone().into()))
                .on_press(move |_: &()| {
                    on_copy();
                    let next = epoch.get_untracked().wrapping_add(1);
                    epoch.set(next);
                    copied.set(true);
                    set_timeout(std::time::Duration::from_millis(1500), move || {
                        if epoch.try_get() == Some(next) {
                            let _ = copied.try_set(false);
                        }
                    });
                })
                .child(dynamic(move || {
                    if copied.get() {
                        super::icon_tinted(Icon::Check, SUCCESS)
                            .w(px(icon_size))
                            .h(px(icon_size))
                    } else {
                        super::icon_tinted(Icon::Copy, MUTED)
                            .w(px(icon_size))
                            .h(px(icon_size))
                    }
                })),
        )
}
