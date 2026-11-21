//! Direct single-choice buttons; selected, hover and keyboard focus stay distinct.
use super::{Icon, icon_tinted, tokens::*};
use nsh_core::prelude::*;

pub fn segment(
    title: &'static str,
    mark: Option<Icon>,
    selected: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    segment_inner(
        title.into(),
        mark,
        CONTROL_HEIGHT,
        BODY_SIZE,
        selected,
        press,
    )
}

pub fn compact_segment(
    title: impl Into<String>,
    mark: Option<Icon>,
    selected: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    segment_inner(title.into(), mark, 28.0, CAPTION_SIZE, selected, press)
}

fn segment_inner(
    title: String,
    mark: Option<Icon>,
    height: f32,
    font_size: f32,
    selected: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    let selected = computed(selected);
    let a11y_title = title.clone();
    let mut button = crate::press_target()
        .app_class(super::motion::CONTROL_TRANSITION)
        .min_h(px(height))
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px_2p5()
        .rounded(px(6.))
        .selected(move || selected.get())
        .bg(move || if selected.get() { SURFACE } else { CLEAR })
        .border_1()
        .border_color(move || if selected.get() { BORDER } else { CLEAR })
        .hover(|s| s.bg(SURFACE))
        .focus(|s| s.border_color(ACCENT))
        .active(|s| s.opacity(0.7))
        .accessibility_label(move || Some(a11y_title.clone().into()))
        .on_press(press);
    if let Some(mark) = mark {
        button = button.child(
            icon_tinted(mark, move || if selected.get() { TEXT } else { MUTED })
                .w(px(14.))
                .h(px(14.)),
        );
    }
    button.child(
        text(title)
            .text_size(px(font_size))
            .font_weight(FontWeight::MEDIUM)
            .text_color(move || if selected.get() { TEXT } else { MUTED }),
    )
}

pub fn segmented() -> View {
    view()
        .self_start()
        .flex_row()
        .flex_wrap()
        .gap_1()
        .p_1()
        .rounded(px(RADIUS))
        .bg(INSET)
}
