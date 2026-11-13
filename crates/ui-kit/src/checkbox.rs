//! Controlled checkbox; NSH owns checked-state accessibility and keyboard input.
use super::{label, tokens::*};
use nsh_core::prelude::*;
pub fn checkbox(
    title: String,
    checked: Signal<bool>,
    change: impl Fn(&bool) + 'static,
) -> nsh_base::CheckboxElement {
    nsh_base::checkbox()
        .checked(move || checked.get())
        .on_change(change)
        .cursor_pointer()
        .disabled_style(|s| s.cursor(Cursor::Default))
        .min_h(px(CONTROL_HEIGHT))
        .px_2()
        .rounded(px(RADIUS))
        .focus(|s| s.bg(INSET))
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(label(move || if checked.get() { "☑" } else { "☐" }))
                .child(label(title)),
        )
}
