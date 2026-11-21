//! NSH owns resizing, minimum sizing and accessibility of split handles.
use super::tokens::*;
use nsh_core::prelude::*;

/// Shared Tailwind indicator classes for NSH horizontal [`nsh_base::ResizablePanelGroup`]
/// handles: a 1pt `BORDER` hairline along the full boundary that highlights and
/// widens to 3pt in `ACCENT` (`ring` / `primary`) while the 9pt handle band is
/// hovered, pressed, or dragged.
pub const HANDLE_CLASS: &str = "w-px bg-border transition-all duration-150 ease-out motion-reduce:transition-none group-hover:w-[3px] group-hover:bg-ring group-active:w-[3px] group-active:bg-primary";

/// Horizontal resizable panel group pre-configured with Canban's shared split
/// handle indicator (`1pt BORDER` idle, `3pt ACCENT` on hover/drag) and
/// accessibility label.
pub fn horizontal_group(label: impl Into<SharedString>) -> nsh_base::ResizablePanelGroupElement {
    nsh_base::h_resizable()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .handle_class(HANDLE_CLASS)
        .handle_accessibility_label(label.into())
}

pub fn split(
    left: AnyElement,
    right: AnyElement,
    width: Signal<f32>,
    min_left: f32,
    min_right: f32,
) -> impl IntoElement {
    horizontal_group("Resize workspace panels")
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
                .disabled(move || !wide.get())
                .handle_class(move || {
                    if wide.get() {
                        Some(SharedString::from(HANDLE_CLASS))
                    } else {
                        Some(SharedString::from("w-0"))
                    }
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
                                            .border_color(move || {
                                                if wide.get() {
                                                    CLEAR.resolve()
                                                } else {
                                                    BORDER.resolve()
                                                }
                                            })
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
    use nsh_core::{ApproxTextEngine, Color, Core, MountItem, NodeId, geometry::size};
    use std::collections::HashMap;

    #[test]
    fn panel_sizing_preserves_user_width_with_readable_minima_and_fullscreen() {
        assert_eq!(pane_extent(1300., 700., false), 600.);
        assert_eq!(pane_extent(1000., 900., false), 340.);
        assert_eq!(pane_extent(1000., 100., false), 460.);
        assert_eq!(pane_extent(720., 700., true), 720.);
    }

    #[test]
    fn horizontal_split_mounts_full_height_theme_reactive_divider_indicator() {
        #[component]
        fn SplitHost() -> impl IntoElement {
            crate::tokens::install_theme();
            let width = signal(420.0_f32);
            view()
                .w(px(900.0))
                .h(px(600.0))
                .child(
                    view()
                        .test_id("switch-theme")
                        .h(px(1.0))
                        .on_press(|| crate::tokens::set_theme("gruvbox")),
                )
                .child(split(
                    view().flex_1().bg(SURFACE).into_element(),
                    view().flex_1().bg(BACKGROUND).into_element(),
                    width,
                    300.0,
                    300.0,
                ))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(900.0), px(600.0)));
        core.mount_root(&nsh_core::element::Root::of::<SplitHost>());

        let mut children_of: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut frames: HashMap<NodeId, Bounds<Points>> = HashMap::new();
        let mut bg_colors: HashMap<NodeId, Color> = HashMap::new();
        let mut handle_band_id: Option<NodeId> = None;
        let mut switch_btn_id: Option<NodeId> = None;

        fn drain(
            core: &mut Core<ApproxTextEngine>,
            children_of: &mut HashMap<NodeId, Vec<NodeId>>,
            frames: &mut HashMap<NodeId, Bounds<Points>>,
            bg_colors: &mut HashMap<NodeId, Color>,
            handle_band_id: &mut Option<NodeId>,
            switch_btn_id: &mut Option<NodeId>,
        ) {
            for _ in 0..6 {
                let mut events = Vec::new();
                core.take_events(&mut events);
                for ev in events {
                    match ev {
                        nsh_core::element::JsEvent::LayoutListener {
                            host,
                            id,
                            listener,
                            frame,
                        } => {
                            core.component_layout(host, id, &listener, frame);
                        }
                        nsh_core::element::JsEvent::Event(id, kind, payload) => {
                            core.component_event(id, kind, &payload);
                        }
                        _ => {}
                    }
                }
                let Some(batch) = core.commit() else {
                    if !core.has_events() {
                        break;
                    }
                    continue;
                };
                for item in &batch.items {
                    match item {
                        MountItem::Insert { parent, child, .. } => {
                            children_of.entry(*parent).or_default().push(*child);
                        }
                        MountItem::Frame { id, frame } => {
                            frames.insert(*id, *frame);
                        }
                        MountItem::ViewStyle { id, style } => {
                            bg_colors.insert(*id, style.background_color);
                        }
                        MountItem::ViewProps { id, props } => {
                            if props.accessibility.role == AccessibilityRole::Adjustable {
                                *handle_band_id = Some(*id);
                            }
                            if props.test_id.as_deref() == Some("switch-theme") {
                                *switch_btn_id = Some(*id);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        drain(
            &mut core,
            &mut children_of,
            &mut frames,
            &mut bg_colors,
            &mut handle_band_id,
            &mut switch_btn_id,
        );

        let band_id = handle_band_id.expect("resizable handle band mounted");
        let band_frame = frames.get(&band_id).expect("band frame");
        assert_eq!(band_frame.size.width, px(9.0));
        assert_eq!(band_frame.size.height, px(599.0));

        let indicator_id = *children_of
            .get(&band_id)
            .and_then(|c| c.first())
            .expect("indicator child inside handle band");
        let indicator_frame = frames.get(&indicator_id).expect("indicator frame");
        assert_eq!(indicator_frame.size.width, px(1.0));
        assert_eq!(
            indicator_frame.size.height,
            px(599.0),
            "handle indicator must stretch across the full split height without collapsing to 0pt"
        );
        let initial_bg = *bg_colors.get(&indicator_id).expect("indicator style");
        assert_ne!(
            initial_bg,
            Color::TRANSPARENT,
            "idle split indicator must paint an opaque semantic BORDER color"
        );

        // Switching the named theme updates the handle indicator's background token.
        core.component_event(
            switch_btn_id.expect("switch-theme button"),
            nsh_core::EventKind::Press,
            &nsh_core::EventPayload::None,
        );
        drain(
            &mut core,
            &mut children_of,
            &mut frames,
            &mut bg_colors,
            &mut handle_band_id,
            &mut switch_btn_id,
        );
        let gruvbox_bg = *bg_colors
            .get(&indicator_id)
            .expect("updated indicator style");
        assert_ne!(
            gruvbox_bg, initial_bg,
            "split handle indicator color updates when Canban theme changes"
        );
    }
}
