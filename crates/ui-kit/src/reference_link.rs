//! Inline reference link with a rich, noninteractive hover/focus preview.
//! Domain resolution and navigation belong to the caller.
use super::tokens::*;
use nsh_core::prelude::*;

pub fn reference_link(
    title: String,
    preview: impl IntoElement,
    open_preview: impl Fn() + 'static,
    navigate: impl Fn() + 'static,
) -> impl IntoElement {
    text(title.clone())
        .role(AccessibilityRole::Link)
        .focusable(true)
        .label(title)
        .text_color(ACCENT)
        .underline()
        .px_1()
        .rounded(px(4.))
        .bg(CLEAR)
        .hover(|s| s.bg(SELECTED))
        .cursor_pointer()
        .on_press(move |_: &PressEvent| navigate())
        .tooltip(
            nsh_base::tooltip()
                .delay(350.)
                .placement(Placement::Top)
                .on_open(open_preview)
                .child(
                    view()
                        .w(px(320.))
                        .max_w(Percentage(0.9))
                        .p_3()
                        .gap_2()
                        .rounded(px(8.))
                        .bg(SURFACE)
                        .border_1()
                        .border_color(BORDER)
                        .shadow_md()
                        .child(preview),
                ),
        )
}
