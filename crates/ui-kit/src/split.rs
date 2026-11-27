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
///
/// Clicking the left pane does not dismiss the right pane. Use
/// [`reveal_split_dismissible`] for master-detail or drawer-style splits where
/// clicking empty space in the left pane should collapse the right pane.
pub fn reveal_split(
    left: AnyElement,
    right: impl Fn() -> AnyElement + 'static,
    width: Signal<f32>,
    full: Memo<bool>,
    requested: Memo<bool>,
) -> impl IntoElement {
    reveal_split_inner(left, right, width, full, requested, None)
}

/// Variant of [`reveal_split`] where clicking an unhandled area of the left
/// pane while side-by-side (`!full`) invokes `on_dismiss` to collapse the right
/// pane.
pub fn reveal_split_dismissible(
    left: AnyElement,
    right: impl Fn() -> AnyElement + 'static,
    width: Signal<f32>,
    full: Memo<bool>,
    requested: Memo<bool>,
    on_dismiss: impl Fn() + 'static,
) -> impl IntoElement {
    reveal_split_inner(
        left,
        right,
        width,
        full,
        requested,
        Some(Box::new(on_dismiss)),
    )
}

fn reveal_split_inner(
    left: AnyElement,
    right: impl Fn() -> AnyElement + 'static,
    width: Signal<f32>,
    full: Memo<bool>,
    requested: Memo<bool>,
    on_dismiss: Option<Box<dyn Fn() + 'static>>,
) -> impl IntoElement {
    let total = signal(0f32);
    let open = computed(move || requested.get() && total.get() > 0.);
    let presence = super::motion::Presence::new(open);
    let last_extent = stored(340f32);
    let extent = computed(move || {
        if open.get() {
            let next = pane_extent(total.get(), width.get(), full.get());
            last_extent.replace(next);
            next
        } else {
            last_extent.get()
        }
    });
    let wide = computed(move || open.get() && !full.get());
    let mut left_pane = view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .overflow_hidden()
        .accessibility_elements_hidden(move || open.get() && full.get())
        .pointer_events(move || {
            if open.get() && full.get() {
                PointerEvents::None
            } else {
                PointerEvents::Auto
            }
        });
    if let Some(on_dismiss) = on_dismiss {
        left_pane = left_pane
            .accessible(false)
            .focusable(false)
            .on_press(move || {
                if wide.get_untracked() {
                    on_dismiss();
                }
            });
    }
    let left_pane = left_pane.child(left);
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
                        .child(left_pane),
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

/// A stable resizable workspace whose left pane reveals and collapses at its
/// fixed target width. Uses NSH native layout transitions (`PANEL_IN` /
/// `PANEL_OUT`) plus `Presence` opacity so content never reflows frame-by-frame
/// while sliding and the right pane (e.g. live native Terminal) smoothly yields
/// or reclaims space.
#[allow(clippy::too_many_arguments)]
pub fn reveal_left_split(
    left: AnyElement,
    right: AnyElement,
    width: Signal<f32>,
    min_left: f32,
    max_left: f32,
    min_right: f32,
    full: Memo<bool>,
    open: Memo<bool>,
    handle_label: impl Into<SharedString>,
    on_resize: impl Fn(f32) + 'static,
) -> impl IntoElement {
    let total = signal(0f32);
    let group_ref = element_ref();
    let presence = super::motion::Presence::new(open);
    let last_extent = stored(width.get_untracked().clamp(min_left, max_left));
    let extent = computed(move || {
        if open.get() {
            let t = total.get();
            let next = if full.get() {
                if t > 0. { t } else { min_left }
            } else if t > 0. {
                width
                    .get()
                    .clamp(min_left, max_left.min((t - min_right).max(min_left)))
            } else {
                width.get().clamp(min_left, max_left)
            };
            last_extent.replace(next);
            next
        } else {
            last_extent.get()
        }
    });
    let prev_open = stored(open.get_untracked());
    effect(move || {
        let is_open = open.get();
        if prev_open.replace(is_open) != is_open {
            let ms = if is_open {
                super::motion::PANEL_IN
            } else {
                super::motion::PANEL_OUT
            };
            super::motion::layout(group_ref, ms);
        }
    });
    let wide = computed(move || open.get() && !full.get());
    view()
        .element_ref(&group_ref)
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .on_layout(move |b: &Bounds<Points>| {
            if (b.size.width.0 - total.get_untracked()).abs() > 0.5 {
                total.set(b.size.width.0);
            }
        })
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
                .handle_accessibility_label(handle_label.into())
                .on_resize(move |sizes: &Vec<Points>| {
                    if wide.get_untracked()
                        && let Some(size) = sizes.first()
                    {
                        let clamped = size.0.max(min_left);
                        width.set(clamped);
                        on_resize(clamped);
                    }
                })
                .child(
                    nsh_base::resizable_panel()
                        .size(move || {
                            if open.get() && !full.get() {
                                Some(px(extent.get()))
                            } else if open.get() && full.get() {
                                None
                            } else {
                                Some(px(0.))
                            }
                        })
                        .min_size(move || px(if wide.get() { min_left } else { 0. }))
                        .max_size(move || {
                            if wide.get() {
                                let t = total.get();
                                Some(px(if t > 0. {
                                    max_left.min((t - min_right).max(min_left))
                                } else {
                                    max_left
                                }))
                            } else if open.get() {
                                None
                            } else {
                                Some(px(0.))
                            }
                        })
                        .fixed(move || !open.get())
                        .child(
                            view()
                                .element_ref(&presence.region)
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
                                .child(
                                    view()
                                        .w(move || px(extent.get()))
                                        .h_full()
                                        .flex_shrink_0()
                                        .child(left),
                                ),
                        ),
                )
                .child(
                    nsh_base::resizable_panel()
                        .size(move || {
                            if open.get() && full.get() {
                                Some(px(0.))
                            } else {
                                None
                            }
                        })
                        .min_size(move || px(if wide.get() { min_right } else { 0. }))
                        .max_size(move || {
                            if open.get() && full.get() {
                                Some(px(0.))
                            } else {
                                None
                            }
                        })
                        .fixed(move || open.get() && full.get())
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.))
                                .min_h(px(0.))
                                .overflow_hidden()
                                .accessibility_elements_hidden(move || open.get() && full.get())
                                .pointer_events(move || {
                                    if open.get() && full.get() {
                                        PointerEvents::None
                                    } else {
                                        PointerEvents::Auto
                                    }
                                })
                                .child(right),
                        ),
                ),
        )
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

    #[test]
    fn reveal_left_split_and_reveal_split_assign_final_widths_on_first_layout() {
        #[component]
        fn RevealLeftHost() -> impl IntoElement {
            crate::tokens::install_theme();
            let width = signal(260.0_f32);
            let open_sig = signal(true);
            let full_sig = signal(false);
            let open = computed(move || open_sig.get());
            let full = computed(move || full_sig.get());
            view()
                .w(px(1200.0))
                .h(px(700.0))
                .child(
                    view()
                        .test_id("toggle-open")
                        .h(px(0.0))
                        .on_press(move || open_sig.set(!open_sig.get_untracked())),
                )
                .child(
                    view()
                        .test_id("toggle-full")
                        .h(px(0.0))
                        .on_press(move || full_sig.set(!full_sig.get_untracked())),
                )
                .child(reveal_left_split(
                    view()
                        .test_id("split-left")
                        .flex_1()
                        .h_full()
                        .into_element(),
                    view()
                        .test_id("split-right")
                        .flex_1()
                        .h_full()
                        .into_element(),
                    width,
                    190.0,
                    544.0,
                    300.0,
                    full,
                    open,
                    "Resize",
                    |_| {},
                ))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.0), px(700.0)));
        core.mount_root(&nsh_core::element::Root::of::<RevealLeftHost>());

        let mut frames: HashMap<NodeId, Bounds<Points>> = HashMap::new();
        let mut test_ids: HashMap<String, NodeId> = HashMap::new();

        fn apply_batch(
            batch: &nsh_core::MountBatch,
            frames: &mut HashMap<NodeId, Bounds<Points>>,
            test_ids: &mut HashMap<String, NodeId>,
        ) {
            for item in &batch.items {
                match item {
                    MountItem::Frame { id, frame } => {
                        frames.insert(*id, *frame);
                    }
                    MountItem::ViewProps { id, props } => {
                        if let Some(tid) = &props.test_id {
                            test_ids.insert(tid.to_string(), *id);
                        }
                    }
                    _ => {}
                }
            }
        }

        fn drain_layout(
            core: &mut Core<ApproxTextEngine>,
            frames: &mut HashMap<NodeId, Bounds<Points>>,
            test_ids: &mut HashMap<String, NodeId>,
        ) {
            for _ in 0..8 {
                let had_batch = if let Some(batch) = core.commit() {
                    apply_batch(&batch, frames, test_ids);
                    true
                } else {
                    false
                };
                let mut events = Vec::new();
                core.take_events(&mut events);
                let mut dispatched = false;
                for ev in events {
                    match ev {
                        nsh_core::element::JsEvent::Event(id, kind, payload) => {
                            core.component_event(id, kind, &payload);
                            dispatched = true;
                        }
                        nsh_core::element::JsEvent::LayoutListener {
                            host,
                            id,
                            listener,
                            frame,
                        } => {
                            core.component_layout(host, id, &listener, frame);
                            dispatched = true;
                        }
                        _ => {}
                    }
                }
                if !had_batch && !dispatched && !core.has_events() {
                    break;
                }
            }
        }

        // 1. VERY FIRST commit (before any on_layout feedback!) must already allocate 1200 - 260 = 940pt.
        let first_batch = core.commit().expect("initial commit batch");
        apply_batch(&first_batch, &mut frames, &mut test_ids);
        let right_id = test_ids["split-right"];
        let left_id = test_ids["split-left"];
        assert_eq!(frames[&left_id].size.width, px(260.0));
        assert_eq!(frames[&right_id].size.width, px(940.0));

        // Drain layout events and verify no second-frame width jump occurs.
        drain_layout(&mut core, &mut frames, &mut test_ids);
        assert_eq!(frames[&right_id].size.width, px(940.0));

        // 2. Collapsing sidebar expands right pane to full 1200pt.
        core.component_event(
            test_ids["toggle-open"],
            nsh_core::EventKind::Press,
            &nsh_core::EventPayload::None,
        );
        drain_layout(&mut core, &mut frames, &mut test_ids);
        assert_eq!(frames[&right_id].size.width, px(1200.0));

        // 3. Reopening in narrow/full mode gives left pane 1200pt and collapses right pane to 0pt.
        core.component_event(
            test_ids["toggle-full"],
            nsh_core::EventKind::Press,
            &nsh_core::EventPayload::None,
        );
        core.component_event(
            test_ids["toggle-open"],
            nsh_core::EventKind::Press,
            &nsh_core::EventPayload::None,
        );
        drain_layout(&mut core, &mut frames, &mut test_ids);
        assert_eq!(frames[&left_id].size.width, px(1200.0));
        assert_eq!(frames[&right_id].size.width, px(0.0));

        // 4. reveal_split resolves extent synchronously on the exact commit `requested` flips true.
        #[component]
        fn RevealRightHost() -> impl IntoElement {
            crate::tokens::install_theme();
            let width = signal(560.0_f32);
            let req = signal(false);
            let full = computed(|| false);
            let requested = computed(move || req.get());
            view()
                .w(px(1200.0))
                .h(px(700.0))
                .child(
                    view()
                        .test_id("open-right")
                        .h(px(0.0))
                        .on_press(move || req.set(true)),
                )
                .child(reveal_split(
                    view().flex_1().into_element(),
                    move || {
                        view()
                            .test_id("reveal-right")
                            .flex_1()
                            .h_full()
                            .into_element()
                    },
                    width,
                    full,
                    requested,
                ))
        }

        let mut r_core = Core::new(ApproxTextEngine, size(px(1200.0), px(700.0)));
        r_core.mount_root(&nsh_core::element::Root::of::<RevealRightHost>());
        let mut r_frames: HashMap<NodeId, Bounds<Points>> = HashMap::new();
        let mut r_ids: HashMap<String, NodeId> = HashMap::new();
        drain_layout(&mut r_core, &mut r_frames, &mut r_ids);
        r_core.component_event(
            r_ids["open-right"],
            nsh_core::EventKind::Press,
            &nsh_core::EventPayload::None,
        );
        if let Some(batch) = r_core.commit() {
            apply_batch(&batch, &mut r_frames, &mut r_ids);
        }
        let rr_id = r_ids["reveal-right"];
        assert_eq!(
            r_frames[&rr_id].size.width,
            px(639.0),
            "reveal_split must mount right pane at exact target extent (1200 - 560 = 640, minus 1pt border = 639), never 340"
        );
    }
}
