//! NSH owns resizing, minimum sizing and accessibility of split handles.
use super::tokens::*;
use nsh_core::prelude::*;
pub fn split(
    left: AnyElement,
    right: AnyElement,
    width: Signal<f32>,
    min_left: f32,
    min_right: f32,
) -> impl IntoElement {
    nsh_base::h_resizable()
        .flex_1()
        .min_h(px(0.0))
        .render_handle(|_: &nsh_base::ResizeHandleContext| view().w(px(1.0)).h_full().bg(BORDER))
        .handle_accessibility_label("Resize workspace panels")
        .on_resize(move |sizes: &Vec<Points>| {
            if let Some(size) = sizes.first() {
                width.set(size.0);
            }
        })
        .child(
            nsh_base::resizable_panel()
                .size(move || px(width.get()))
                .min_size(px(min_left))
                .child(left),
        )
        .child(
            nsh_base::resizable_panel()
                .min_size(px(min_right))
                .child(right),
        )
}
