//! Direct single-choice buttons; selected, hover and keyboard focus stay distinct.
use super::{Icon, icon_tinted, tokens::*};
use nsh_core::prelude::*;

pub fn segment(
    title: &'static str,
    mark: Option<Icon>,
    selected: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    let selected = computed(selected);
    let mut button = nsh_base::button()
        .min_h(px(CONTROL_HEIGHT))
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .px_3()
        .rounded(px(6.))
        .selected(move || selected.get())
        .bg(move || if selected.get() { SURFACE } else { CLEAR })
        .border_1()
        .border_color(move || if selected.get() { BORDER } else { CLEAR })
        .hover(|s| s.bg(SURFACE))
        .focus(|s| s.border_color(ACCENT))
        .active(|s| s.opacity(0.7))
        .accessibility_label(move || Some(title.into()))
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
            .text_size(px(BODY_SIZE))
            .font_weight(FontWeight::MEDIUM)
            .text_color(move || if selected.get() { TEXT } else { MUTED }),
    )
}

pub fn segmented() -> View {
    view()
        .self_start()
        .flex_row()
        .gap_1()
        .p_1()
        .rounded(px(RADIUS))
        .bg(INSET)
}
