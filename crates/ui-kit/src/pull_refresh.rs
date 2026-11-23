//! Pull-to-refresh scroll host and native-animated top indicator.
use super::{Icon, icon_tinted, loading_spinner, motion, tokens::*};
use nsh_core::element::Scroll;
use nsh_core::prelude::*;
use std::time::Duration;

/// Default pull threshold and held top room (in points) for pull-to-refresh.
pub const PULL_REFRESH_HEIGHT: f32 = 48.0;
const WHEEL_OVERSCROLL_THRESHOLD: f32 = 80.0;
const WHEEL_OVERSCROLL_WINDOW_MS: u128 = 600;
const SETTLE_HOLD_MS: u64 = 320;

/// Native-animated top pull-to-refresh indicator driven by a scroll view's `pull_value`.
///
/// - While pulling (`0..height`), fades in, glides into the opened gap, and rotates an
///   arrow from `0° -> 180°` while highlighting the badge as it crosses `height`.
/// - While `refreshing()` is true (and during the brief `320ms` bounce-back settle), renders
///   Canban's `loading_spinner` in `ACCENT` so the spinner never snaps back to an arrow mid-rebound.
pub fn pull_refresh_indicator(
    pull: AnimatedValue,
    refreshing: impl Fn() -> bool + Copy + 'static,
    height: f32,
) -> View {
    let height = height.max(1.0);
    let refreshing = computed(refreshing);
    let settling = signal(false);
    let settle_ticket = signal(0_u64);
    let prev_refreshing = stored(refreshing.get_untracked());

    effect(move || {
        let active = refreshing.get();
        let was = prev_refreshing.replace(active);
        if active {
            settle_ticket.update(|n| *n += 1);
            settling.set(false);
        } else if was {
            let next = settle_ticket.get_untracked() + 1;
            settle_ticket.set(next);
            settling.set(true);
            set_timeout(Duration::from_millis(SETTLE_HOLD_MS), move || {
                if settle_ticket.try_get() == Some(next) {
                    settling.set(false);
                }
            });
        }
    });

    let reduced = motion::reduced();
    let mut overlay = view()
        .pointer_events(PointerEvents::None)
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .h(px(height))
        .items_center()
        .justify_center()
        .bind_opacity(&pull.interpolate(&[4.0, height * 0.65], &[0.0, 1.0]));

    if !reduced {
        overlay = overlay
            .bind_translate_y(&pull.interpolate(&[0.0, height], &[-height * 0.35, 0.0]))
            .bind_scale(&pull.interpolate(&[0.0, height], &[0.78, 1.0]));
    }

    overlay.child(dynamic(move || {
        let active = refreshing.get();
        if active || settling.get() {
            let mut pill = view()
                .w(px(28.))
                .h(px(28.))
                .rounded_full()
                .bg(SELECTED)
                .border_1()
                .border_color(ACCENT)
                .items_center()
                .justify_center()
                .child(loading_spinner(14., ACCENT));
            if active {
                pill = pill.label("Refreshing");
            }
            pill.into_element()
        } else {
            let mut arrow = view()
                .relative()
                .w(px(14.))
                .h(px(14.))
                .items_center()
                .justify_center()
                .child(
                    view()
                        .absolute()
                        .top(px(0.))
                        .left(px(0.))
                        .bind_opacity(&pull.interpolate(&[height * 0.85, height], &[1.0, 0.0]))
                        .child(icon_tinted(Icon::ArrowDown, MUTED).w(px(14.)).h(px(14.))),
                )
                .child(
                    view()
                        .absolute()
                        .top(px(0.))
                        .left(px(0.))
                        .bind_opacity(&pull.interpolate(&[height * 0.85, height], &[0.0, 1.0]))
                        .child(icon_tinted(Icon::ArrowDown, ACCENT).w(px(14.)).h(px(14.))),
                );
            if !motion::reduced() {
                arrow =
                    arrow.bind_rotate(&pull.interpolate(&[height * 0.5, height], &[0.0, 180.0]));
            }
            view()
                .w(px(28.))
                .h(px(28.))
                .rounded_full()
                .border_1()
                .bind_background(
                    &pull.interpolate_colors(&[height * 0.85, height], &[INSET, SELECTED]),
                )
                .bind_border_color(
                    &pull.interpolate_colors(&[height * 0.85, height], &[BORDER, ACCENT]),
                )
                .items_center()
                .justify_center()
                .child(arrow)
                .into_element()
        }
    }))
}

/// Wraps a vertical `scroll()` with unified pull-to-refresh behavior:
/// - Trackpad pull-to-refresh (`pull_value` + `refresh_indicator` + `on_refresh`)
/// - Discrete mouse wheel overscroll past the top edge (`offset.y <= 0.5`)
/// - Floating [`pull_refresh_indicator`] centered in the opened top gap.
///
/// Attaches `scroll_ref` to `scroll_el` so callers can keep using `scroll_ref` for
/// `scroll_to` or `watch_scroll` without clobbering `.on_scroll(...)`.
pub fn pull_refresh_scroll(
    scroll_el: Scroll,
    scroll_ref: ElementRef,
    refreshing: impl Fn() -> bool + Copy + 'static,
    on_refresh: impl Fn() + Copy + 'static,
) -> View {
    if cfg!(target_os = "ios") {
        return view()
            .relative()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .child(
                scroll_el
                    .flex_1()
                    .min_h(px(0.))
                    .element_ref(&scroll_ref)
                    .refreshing(refreshing)
                    .on_refresh(move |_: &()| on_refresh()),
            );
    }

    let pull = animated_value(0.0);
    let last_offset_y = stored(Option::<f32>::None);
    let wheel_overscroll = stored((0.0_f32, nsh_core::time::now()));

    watch_scroll(
        move || scroll_ref.node(),
        move |e: &ScrollEvent| {
            last_offset_y.replace(Some(e.offset.y.0));
        },
    );

    view()
        .relative()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .child(
            scroll_el
                .flex_1()
                .min_h(px(0.))
                .element_ref(&scroll_ref)
                .pull_value(&pull)
                .refresh_indicator(Some(px(PULL_REFRESH_HEIGHT)))
                .refreshing(refreshing)
                .on_refresh(move |_: &()| on_refresh())
                .on_wheel(move |ev: &nsh_core::WheelEvent| {
                    if ev.delta.precise() || untracked(refreshing) {
                        return;
                    }
                    let dy = ev.delta.points(px(20.)).y.0;
                    let at_top = last_offset_y.get().is_none_or(|y| y <= 0.5);
                    if dy < 0.0 && at_top {
                        let now = nsh_core::time::now();
                        let (prev, ts) = wheel_overscroll.get();
                        let accum = if now.saturating_duration_since(ts).as_millis()
                            <= WHEEL_OVERSCROLL_WINDOW_MS
                        {
                            prev + (-dy)
                        } else {
                            -dy
                        };
                        if accum >= WHEEL_OVERSCROLL_THRESHOLD {
                            wheel_overscroll.replace((0.0, now));
                            on_refresh();
                        } else {
                            wheel_overscroll.replace((accum, now));
                        }
                    } else if dy > 0.0 {
                        wheel_overscroll.replace((0.0, nsh_core::time::now()));
                    }
                }),
        )
        .child(pull_refresh_indicator(
            pull,
            refreshing,
            PULL_REFRESH_HEIGHT,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::{
        ApproxTextEngine, Core, EventKind, EventPayload, MountItem, NodeId, WheelDelta, WheelEvent,
        geometry::{point, size},
    };
    use std::collections::HashMap;

    #[derive(Default)]
    struct Tree {
        labels: HashMap<String, NodeId>,
        texts: HashMap<NodeId, String>,
        scroll_nodes: Vec<NodeId>,
    }

    fn pump(core: &mut Core<ApproxTextEngine>, tree: &mut Tree, now: nsh_core::time::Instant) {
        for _ in 0..8 {
            core.run_frame(now);
            core.run_component_tasks();
            let Some(batch) = core.commit() else {
                break;
            };
            for item in &batch.items {
                match item {
                    MountItem::Create { id, kind, .. } => {
                        if *kind == nsh_core::ViewKind::Scroll {
                            tree.scroll_nodes.push(*id);
                        }
                    }
                    MountItem::ViewProps { id, props } => {
                        if let Some(label) = &props.accessibility.label {
                            tree.labels.insert(label.to_string(), *id);
                        }
                    }
                    MountItem::Text { id, text, .. } => {
                        tree.texts.insert(*id, text.text.to_string());
                    }
                    MountItem::Delete { id } => {
                        tree.scroll_nodes.retain(|n| *n != *id);
                        tree.texts.remove(id);
                        tree.labels.retain(|_, v| *v != *id);
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn pull_refresh_scroll_handles_refresh_event_wheel_overscroll_and_settle_hold() {
        #[component]
        fn Probe() -> impl IntoElement {
            install_theme();
            let refreshing = signal(false);
            let count = signal(0_u32);
            let scroll_ref = element_ref();
            view()
                .w(px(320.))
                .h(px(400.))
                .child(
                    crate::press_target()
                        .accessibility_label("stop-refresh")
                        .on_press(move |_: &()| refreshing.set(false)),
                )
                .child(crate::label(move || format!("count:{}", count.get())))
                .child(pull_refresh_scroll(
                    scroll().label("probe-scroll").child(
                        view()
                            .gap_2()
                            .children((0..40).map(|i| crate::label(format!("Row {i}")))),
                    ),
                    scroll_ref,
                    move || refreshing.get(),
                    move || {
                        count.update(|n| *n += 1);
                        refreshing.set(true);
                    },
                ))
        }

        let clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
        let _clock = clock.enter();
        let mut core = Core::new(ApproxTextEngine, size(px(320.), px(400.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<Probe>());
        let mut tree = Tree::default();
        pump(&mut core, &mut tree, nsh_core::time::now());

        let scroll_id = tree.labels["probe-scroll"];
        assert!(!tree.labels.contains_key("Refreshing"));
        assert!(tree.texts.values().any(|t| t == "count:0"));

        // 1. Native trackpad pull-to-refresh event enters refreshing state.
        assert!(core.component_event(scroll_id, EventKind::Refresh, &EventPayload::None));
        clock.advance_to(nsh_core::time::now() + Duration::from_millis(16));
        pump(&mut core, &mut tree, nsh_core::time::now());
        assert!(tree.labels.contains_key("Refreshing"));
        assert!(tree.texts.values().any(|t| t == "count:1"));

        // 2. Stopping refresh clears the "Refreshing" label and completes the settle hold after 320ms.
        assert!(core.component_event(
            tree.labels["stop-refresh"],
            EventKind::Press,
            &EventPayload::None
        ));
        clock.advance_to(nsh_core::time::now() + Duration::from_millis(16));
        pump(&mut core, &mut tree, nsh_core::time::now());
        assert!(!tree.labels.contains_key("Refreshing"));
        clock.advance_to(nsh_core::time::now() + Duration::from_millis(350));
        pump(&mut core, &mut tree, nsh_core::time::now());

        // 3. Discrete mouse wheel overscroll while scrolled down does NOT trigger refresh.
        core.set_scroll_offset(scroll_id, point(px(0.), px(120.)), u32::MAX);
        clock.advance_to(nsh_core::time::now() + Duration::from_millis(16));
        pump(&mut core, &mut tree, nsh_core::time::now());
        let wheel_up = EventPayload::Wheel(WheelEvent::new(
            point(px(100.), px(100.)),
            point(px(100.), px(100.)),
            WheelDelta::Lines(point(0., -5.)),
            Default::default(),
        ));
        assert!(core.component_event(scroll_id, EventKind::Wheel, &wheel_up));
        clock.advance_to(nsh_core::time::now() + Duration::from_millis(16));
        pump(&mut core, &mut tree, nsh_core::time::now());
        assert!(!tree.labels.contains_key("Refreshing"));
        assert!(tree.texts.values().any(|t| t == "count:1"));

        // 4. Discrete mouse wheel overscroll at top (`offset.y <= 0.5`) triggers refresh.
        core.set_scroll_offset(scroll_id, point(px(0.), px(0.)), u32::MAX);
        clock.advance_to(nsh_core::time::now() + Duration::from_millis(16));
        pump(&mut core, &mut tree, nsh_core::time::now());
        assert!(core.component_event(scroll_id, EventKind::Wheel, &wheel_up));
        clock.advance_to(nsh_core::time::now() + Duration::from_millis(16));
        pump(&mut core, &mut tree, nsh_core::time::now());
        assert!(tree.labels.contains_key("Refreshing"));
        assert!(tree.texts.values().any(|t| t == "count:2"));
    }
}
