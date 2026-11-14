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
