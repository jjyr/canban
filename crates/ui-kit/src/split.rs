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
        .render_handle(|_: &nsh_base::ResizeHandleContext| {
            view()
                .w(px(5.0))
                .h_full()
                .items_center()
                .justify_center()
                .hover(|s| s.bg(ACCENT))
                .active(|s| s.bg(ACCENT))
                .child(view().w(px(1.0)).h_full().bg(BORDER))
        })
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

/// A stable resizable workspace whose right pane reveals at its final text
/// width. The native layout transition moves the pane boundary and the left
/// viewport; it never feeds frame-by-frame widths back into document layout.
pub fn reveal_split(
    left: AnyElement,
    right: impl Fn() -> AnyElement + 'static,
    width: Signal<f32>,
    full: Memo<bool>,
    requested: Memo<bool>,
    on_dismiss: impl Fn() + 'static,
) -> impl IntoElement {
    let total = signal(0f32);
    let open = computed(move || requested.get() && total.get() > 0.);
    let presence = super::motion::Presence::new(open);
    let extent = signal(340f32);
    effect(move || {
        if open.get() {
            extent.set(pane_extent(total.get(), width.get(), full.get()));
        }
    });
    let wide = computed(move || open.get() && !full.get());
    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .on_layout(move |b: &Bounds<Points>| total.set(b.size.width.0))
        .child(
            nsh_base::h_resizable()
                .flex_1()
                .min_h(px(0.))
                .min_w(px(0.))
                .render_handle(move |_: &nsh_base::ResizeHandleContext| {
                    view()
                        .w(move || px(if wide.get() { 5. } else { 0. }))
                        .h_full()
                        .items_center()
                        .justify_center()
                        .hover(|s| s.bg(ACCENT))
                        .active(|s| s.bg(ACCENT))
                        .child(
                            view()
                                .w(move || px(if wide.get() { 1. } else { 0. }))
                                .h_full()
                                .bg(BORDER),
                        )
                })
                .handle_accessibility_label("Resize workspace panels")
                .on_resize(move |sizes: &Vec<Points>| {
                    if wide.get_untracked()
                        && let Some(size) = sizes.first()
                    {
                        width.set(size.0);
                    }
                })
                .child(
                    nsh_base::resizable_panel()
                        .size(move || {
                            px((total.get() - if open.get() { extent.get() } else { 0. }).max(0.))
                        })
                        .min_size(move || px(if wide.get() { 540. } else { 0. }))
                        .max_size(move || {
                            px(if open.get() && full.get() {
                                0.
                            } else {
                                total.get()
                            })
                        })
                        .fixed(move || !wide.get())
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.))
                                .min_h(px(0.))
                                .overflow_hidden()
                                .accessible(false)
                                .focusable(false)
                                .accessibility_elements_hidden(move || open.get() && full.get())
                                .pointer_events(move || {
                                    if open.get() && full.get() {
                                        PointerEvents::None
                                    } else {
                                        PointerEvents::Auto
                                    }
                                })
                                .on_press(move || {
                                    if wide.get_untracked() {
                                        on_dismiss();
                                    }
                                })
                                .child(left),
                        ),
                )
                .child(
                    nsh_base::resizable_panel()
                        .size(move || px(if open.get() { extent.get() } else { 0. }))
                        .min_size(move || px(if wide.get() { 340. } else { 0. }))
                        .max_size(move || {
                            px(if wide.get() {
                                (total.get() - 540.).max(340.)
                            } else if open.get() {
                                total.get()
                            } else {
                                0.
                            })
                        })
                        .fixed(move || !wide.get())
                        .child(
                            view()
                                .element_ref(&presence.region)
                                .on_touch(|_: &nsh_core::TouchEvent| {})
                                .flex_1()
                                .min_w(px(0.))
                                .min_h(px(0.))
                                .overflow_hidden()
                                .accessibility_elements_hidden(move || !open.get())
                                .pointer_events(move || {
                                    if open.get() {
                                        PointerEvents::Auto
                                    } else {
                                        PointerEvents::None
                                    }
                                })
                                .bind_opacity(&presence.opacity)
                                .child(when(
                                    move || presence.mounted.get(),
                                    move || {
                                        view()
                                            .w(move || px(extent.get()))
                                            .h_full()
                                            .flex_shrink_0()
                                            .border_l_1()
                                            .border_color(BORDER)
                                            .child(right())
                                    },
                                )),
                        ),
                ),
        )
}
fn pane_extent(total: f32, left: f32, full: bool) -> f32 {
    if full {
        total.max(0.)
    } else {
        (total - left).clamp(340., (total - 540.).max(340.))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_sizing_preserves_user_width_with_readable_minima_and_fullscreen() {
        assert_eq!(pane_extent(1300., 700., false), 600.);
        assert_eq!(pane_extent(1000., 900., false), 340.);
        assert_eq!(pane_extent(1000., 100., false), 460.);
        assert_eq!(pane_extent(720., 700., true), 720.);
    }
}
