//! Shared settings section/field geometry and section jump-rail controls.
//! The screen owns one outer scroll.
use super::{SectionGroup, label, muted, press_target, section_group, tokens::*};
use nsh_core::prelude::*;

pub fn settings_section(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
) -> SectionGroup {
    section_group(title).description(description).body_gap(14.0)
}

pub fn settings_field(title: &str, description: &str, control: impl IntoElement) -> View {
    let mut field = view()
        .min_w(px(0.))
        .gap_1p5()
        .child(label(title.to_owned()).font_weight(FontWeight::MEDIUM))
        .child(control);
    if !description.trim().is_empty() {
        field = field.child(muted(description.to_owned()));
    }
    field
}

/// Group caption inside the Settings jump column (e.g. `INTERFACE`, `EXECUTION`).
pub fn settings_nav_heading(title: impl Into<SharedString>) -> View {
    view().px_2p5().pt_2().pb_0p5().child(
        text(title.into())
            .text_size(px(10.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(MUTED),
    )
}

/// Accessible row inside the Settings jump column. Keeps selected, hover, and
/// keyboard focus distinct while reserving a leading accent bar for active state.
pub fn settings_nav_item(
    title: impl Into<SharedString>,
    selected: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    let title: SharedString = title.into();
    let a11y_title = title.clone();
    let selected = computed(selected);
    press_target()
        .w_full()
        .min_h(px(28.))
        .px_2()
        .py_1()
        .rounded(px(6.))
        .flex_row()
        .items_center()
        .gap_2()
        .selected(move || selected.get())
        .bg(move || if selected.get() { SELECTED } else { CLEAR })
        .border_1()
        .border_color(CLEAR)
        .hover(move |s| {
            if selected.get() {
                s.bg(SELECTED)
            } else {
                s.bg(INSET)
            }
        })
        .focus(|s| s.border_color(ACCENT))
        .active(|s| s.opacity(0.75))
        .accessibility_label(move || Some(a11y_title.clone()))
        .on_press(press)
        .child(
            view()
                .w(px(2.5))
                .h(px(14.))
                .rounded_full()
                .flex_shrink_0()
                .bg(move || if selected.get() { ACCENT } else { CLEAR }),
        )
        .child(
            text(title)
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || if selected.get() { TEXT } else { MUTED })
                .truncate()
                .min_w(px(0.))
                .flex_1(),
        )
}
