//! Canban's custom overlay scrollbars.
//!
//! Replaces the default bright-white macOS platform scroller with a quiet,
//! theme-reactive pill indicator across all `scroll()` regions, plus an
//! explicitly placeable [`vertical_scrollbar`] for dense Card lists (such as
//! Board stage columns) where the rail sits in the outer gutter clear of Card
//! borders and hover actions.
//!
//! Pass-through overlay containers always carry `.accessible(false)` alongside
//! `.pointer_events(PointerEvents::BoxNone)` / `None` so `nsh-core` emits
//! `MountItem::ViewProps` on initial view creation rather than leaving newly
//! mounted full-viewport decoration hosts in `PointerEvents::Auto`.
use super::{motion, tokens::*};
use nsh_base::ScrollbarTarget;
use nsh_core::{
    element::{AnyValue, BuiltInScrollbar, NodeRef, set_builtin_scrollbars},
    geometry::{Along, Axis},
    prelude::*,
    time::{Instant, now},
};
use std::time::Duration;

const TRACK_SPAN: f32 = 9.0;
const RAIL_PAD_ALONG: f32 = 3.0;
const MIN_THUMB: f32 = 32.0;
const THUMB_IDLE: f32 = 3.5;
const THUMB_ACTIVE: f32 = 5.0;
const ENTER_MS: f32 = 120.0;
const EXIT_MS: f32 = 220.0;
const IDLE_MS: u64 = 850;

pub(crate) fn install() {
    set_builtin_scrollbars(built_in, Some(AnyValue::new(())));
}

/// Explicitly placed vertical scrollbar bound to `scroll` (`scroll().scrollbar(false)`).
/// Callers position the returned component element (for example in a column's
/// outer gutter via `.absolute().top_0().bottom_0().right(px(-8.))`).
pub fn vertical_scrollbar(scroll: impl Into<ScrollbarTarget>) -> ScrollbarRailElement {
    scrollbar_rail()
        .target(scroll.into())
        .horizontal(false)
        .w(px(TRACK_SPAN))
}

fn built_in(of: &BuiltInScrollbar) -> Option<AnyElement> {
    Some(
        viewport_scrollbar()
            .target(ScrollbarTarget::Node(NodeRef(of.target)))
            .horizontal(of.horizontal)
            .into_element(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Props)]
struct ViewportScrollbarProps {
    #[prop(default)]
    target: ScrollbarTarget,
    #[prop(default)]
    horizontal: bool,
}

#[component]
fn ViewportScrollbar(props: ViewportScrollbarProps) -> impl IntoElement {
    let bar = scrollbar_rail()
        .target(props.target)
        .horizontal(props.horizontal)
        .absolute()
        .map(|el| {
            if props.horizontal {
                el.left_0().right_0().bottom_0().h(px(TRACK_SPAN))
            } else {
                el.top_0().bottom_0().right_0().w(px(TRACK_SPAN))
            }
        });
    view()
        .accessible(false)
        .pointer_events(PointerEvents::BoxNone)
        .child(bar)
}

#[derive(Debug, Clone, Copy, PartialEq, Props)]
pub struct ScrollbarRailProps {
    #[prop(default)]
    pub target: ScrollbarTarget,
    #[prop(default)]
    pub horizontal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    from: f32,
}

#[derive(Clone, Copy)]
struct Ctl {
    target: ScrollbarTarget,
    axis: Axis,
    metrics: Signal<Option<ScrollEvent>>,
    rail: Signal<f32>,
    rail_origin: Stored<Point<Points>>,
    active: Signal<bool>,
    over_track: Signal<bool>,
    over_thumb: Signal<bool>,
    drag: Signal<Option<Drag>>,
    deadline: Stored<Option<Instant>>,
    opacity: AnimatedValue,
    fade_target: Stored<f32>,
}

impl Ctl {
    fn node(self) -> Option<nsh_core::NodeId> {
        self.target.node()
    }

    fn along(self) -> Option<(f32, f32, f32)> {
        let m = self.metrics.get()?;
        Some((
            m.size.along(self.axis).0,
            m.content_size.along(self.axis).0,
            m.offset.along(self.axis).0,
        ))
    }

    fn range(self) -> f32 {
        self.along()
            .map_or(0.0, |(view, content, _)| (content - view).max(0.0))
    }

    fn scrolls(self) -> bool {
        self.range() > 1.0
    }

    fn thumb(self) -> f32 {
        let rail = self.rail.get();
        let Some((view, content, _)) = self.along() else {
            return rail;
        };
        let min = MIN_THUMB.min(rail);
        (rail * view / content.max(1.0)).clamp(min, rail)
    }

    fn position(self) -> f32 {
        let range = self.range();
        let Some((_, _, offset)) = self.along() else {
            return 0.0;
        };
        if range <= 0.0 {
            return 0.0;
        }
        (self.rail.get() - self.thumb()).max(0.0) * (offset / range).clamp(0.0, 1.0)
    }

    fn offset_at(self, position: f32) -> f32 {
        untracked(|| {
            let travel = (self.rail.get() - self.thumb()).max(0.0);
            if travel <= 0.0 {
                return 0.0;
            }
            (position / travel).clamp(0.0, 1.0) * self.range()
        })
    }

    fn scroll_to(self, offset: f32) {
        let Some(node) = self.node() else { return };
        let Some(current) = self.metrics.get_untracked() else {
            return;
        };
        let mut to = current.offset;
        match self.axis {
            Axis::Horizontal => to.x = px(offset),
            Axis::Vertical => to.y = px(offset),
        }
        NodeRef(node).scroll_to(to, false);
        self.metrics
            .set(Some(ScrollEvent { offset: to, ..current }));
    }

    fn touch(self) {
        if !self.active.get_untracked() {
            self.active.set(true);
        }
        let delay = Duration::from_millis(IDLE_MS);
        if self.deadline.replace(Some(now() + delay)).is_none() {
            self.hide_after(delay);
        }
    }

    fn hide_after(self, delay: Duration) {
        set_timeout(delay, move || {
            let Some(deadline) = self.deadline.get() else {
                return;
            };
            let left = deadline.saturating_duration_since(now());
            if left.is_zero() {
                self.deadline.replace(None);
                self.active.set(false);
            } else {
                self.hide_after(left);
            }
        });
    }

    fn shown(self) -> bool {
        self.scrolls()
            && (self.drag.get().is_some() || self.over_track.get() || self.active.get())
    }

    fn fade_to(self, visible: bool, first: bool) {
        let target = if visible { 1.0 } else { 0.0 };
        if first || motion::reduced() {
            self.fade_target.replace(target);
            self.opacity.set(target);
            return;
        }
        if (self.fade_target.replace(target) - target).abs() < f32::EPSILON {
            return;
        }
        let (ms, easing) = if visible {
            (ENTER_MS, Easing::EaseOut)
        } else {
            (EXIT_MS, Easing::Curve(Curve::Cubic, EaseMode::In))
        };
        drop(self.opacity.timing(target, ms, easing));
    }

    fn hover_track(self, over: bool) {
        self.over_track.set(over);
        if over || self.active.get_untracked() {
            self.touch();
        }
    }

    fn press_track(self, at: Point<Points>) {
        let along = at.along(self.axis).0 - self.rail_origin.get().along(self.axis).0;
        let thumb = untracked(|| self.thumb());
        let offset = self.offset_at(along - thumb / 2.0);
        self.drag.set(Some(Drag { from: offset }));
        self.scroll_to(offset);
    }

    fn press_thumb(self) {
        let from = untracked(|| self.along()).map_or(0.0, |(_, _, o)| o);
        self.drag.set(Some(Drag {
            from: from.clamp(0.0, untracked(|| self.range())),
        }));
    }

    fn drag_by(self, delta: f32) {
        let Some(drag) = self.drag.get_untracked() else {
            return;
        };
        let (travel, range) =
            untracked(|| ((self.rail.get() - self.thumb()).max(0.0), self.range()));
        if travel <= 0.0 {
            return;
        }
        let offset = (drag.from + delta * range / travel).clamp(0.0, range);
        self.scroll_to(offset);
    }

    fn release(self) {
        if self.drag.get_untracked().is_some() {
            self.drag.set(None);
            self.touch();
        }
    }
}

fn thumb_fill(emphasized: bool) -> AdaptiveColor {
    let base = MUTED.resolve().dark;
    let color = base.alpha(if emphasized { 0.72 } else { 0.38 });
    AdaptiveColor {
        light: color,
        dark: color,
    }
}

#[component]
pub fn ScrollbarRail(props: ScrollbarRailProps, _cx: Scope<NoEvent>) -> impl IntoElement {
    let axis = if props.horizontal {
        Axis::Horizontal
    } else {
        Axis::Vertical
    };
    let ctl = Ctl {
        target: props.target,
        axis,
        metrics: signal(None),
        rail: signal(0.0),
        rail_origin: stored(Point::default()),
        active: signal(false),
        over_track: signal(false),
        over_thumb: signal(false),
        drag: signal(None),
        deadline: stored(None),
        opacity: animated_value(0.0),
        fade_target: stored(0.0),
    };
    watch_scroll(
        move || ctl.node(),
        move |e: &ScrollEvent| {
            let before = ctl.metrics.get_untracked();
            let mut next = *e;
            if let Some(prev) = before.filter(|_| ctl.drag.get_untracked().is_some()) {
                match axis {
                    Axis::Horizontal => next.offset.x = prev.offset.x,
                    Axis::Vertical => next.offset.y = prev.offset.y,
                }
            }
            ctl.metrics.set(Some(next));
            if before.is_some_and(|b| b.offset != next.offset) {
                ctl.touch();
            }
        },
    );
    let shown = computed(move || ctl.shown());
    let first = std::cell::Cell::new(true);
    effect(move || {
        let visible = shown.get();
        ctl.fade_to(visible, first.replace(false));
    });
    let interactive = computed(move || ctl.scrolls());
    let inner_pe = move || {
        if interactive.get() {
            PointerEvents::BoxNone
        } else {
            PointerEvents::None
        }
    };
    let emphasized = computed(move || {
        ctl.drag.get().is_some() || ctl.over_thumb.get() || ctl.over_track.get()
    });
    let thumb_cross = move || {
        Length::from(px(if emphasized.get() {
            THUMB_ACTIVE
        } else {
            THUMB_IDLE
        }))
    };
    let thumb = view()
        .accessible(false)
        .rounded_full()
        .cursor_pointer()
        .bg(move || thumb_fill(emphasized.get()))
        .map(|v| match axis {
            Axis::Vertical => v.h_full().w(thumb_cross),
            Axis::Horizontal => v.w_full().h(thumb_cross),
        })
        .on_mouse_enter(move |_: &MouseEvent| ctl.over_thumb.set(true))
        .on_mouse_leave(move |_: &MouseEvent| ctl.over_thumb.set(false))
        .claim_touches(TouchClaim::on_start().ask(|request: &ClaimRequest| request.claim()))
        .keep_touches(true)
        .on_responder_grant(move |_: &ResponderEvent| ctl.press_thumb())
        .on_responder_move(move |e: &ResponderEvent| ctl.drag_by(e.gesture.delta.along(axis).0))
        .on_responder_release(move |_: &ResponderEvent| ctl.release())
        .on_responder_terminate(move |_: &ResponderEvent| ctl.release());

    let holder = view()
        .accessible(false)
        .flex_shrink_0()
        .pointer_events(inner_pe)
        .map(|v| match axis {
            Axis::Vertical => v
                .w_full()
                .items_end()
                .pr(px(1.5))
                .h(move || Length::from(px(ctl.thumb())))
                .translate_y(move || DefiniteLength::from(px(ctl.position()))),
            Axis::Horizontal => v
                .h_full()
                .justify_end()
                .pb(px(1.5))
                .w(move || Length::from(px(ctl.thumb())))
                .translate_x(move || DefiniteLength::from(px(ctl.position()))),
        })
        .child(thumb);

    let rail = view()
        .accessible(false)
        .flex_1()
        .pointer_events(inner_pe)
        .map(|v| match axis {
            Axis::Vertical => v.flex_col(),
            Axis::Horizontal => v.flex_row(),
        })
        .on_layout(move |frame: &Bounds<Points>| {
            ctl.rail_origin.replace(frame.origin);
            ctl.rail.set(frame.size.along(axis).0);
        })
        .child(holder);

    view()
        .accessible(false)
        .overflow_hidden()
        .cursor_pointer()
        .map(|v| match axis {
            Axis::Vertical => v.flex_col().py(px(RAIL_PAD_ALONG)),
            Axis::Horizontal => v.flex_row().px(px(RAIL_PAD_ALONG)),
        })
        .on_mouse_enter(move |_: &MouseEvent| ctl.hover_track(true))
        .on_mouse_leave(move |_: &MouseEvent| ctl.hover_track(false))
        .claim_touches(TouchClaim::on_start().ask(|request: &ClaimRequest| request.claim()))
        .keep_touches(true)
        .on_responder_grant(move |e: &ResponderEvent| ctl.press_track(e.location))
        .on_responder_move(move |e: &ResponderEvent| ctl.drag_by(e.gesture.delta.along(axis).0))
        .on_responder_release(move |_: &ResponderEvent| ctl.release())
        .on_responder_terminate(move |_: &ResponderEvent| ctl.release())
        .bind_opacity(&ctl.opacity)
        .pointer_events(move || {
            if interactive.get() {
                PointerEvents::Auto
            } else {
                PointerEvents::None
            }
        })
        .child(rail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::{ApproxTextEngine, Core, MountItem, NodeId, ROOT_ID, Size, geometry::size};
    use std::collections::HashMap;

    #[component]
    fn HostWithScroll() -> impl IntoElement {
        crate::tokens::install_theme();
        let col_scroll = element_ref();
        view()
            .w(px(320.0))
            .h(px(200.0))
            .gap_3()
            .child(
                scroll()
                    .w(px(300.0))
                    .h(px(120.0))
                    .child(view().w_full().h(px(640.0))),
            )
            .child(
                view()
                    .relative()
                    .w(px(260.0))
                    .h(px(120.0))
                    .child(
                        scroll()
                            .element_ref(&col_scroll)
                            .scrollbar(false)
                            .flex_1()
                            .child(view().w_full().h(px(480.0))),
                    )
                    .child(
                        vertical_scrollbar(col_scroll)
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(-8.0)),
                    ),
            )
    }

    #[test]
    fn builtin_scrollbar_host_emits_box_none_view_props_on_initial_mount() {
        let mut core = Core::new(ApproxTextEngine, size(px(800.0), px(600.0)));
        core.mount_root(&nsh_core::element::Root::of::<HostWithScroll>());
        let batch = core.commit().expect("initial mount batch");

        let mut scroll_nodes = Vec::new();
        let mut children_of: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut pointer_events: HashMap<NodeId, PointerEvents> = HashMap::new();
        let mut frames: HashMap<NodeId, Bounds<Points>> = HashMap::new();

        for item in &batch.items {
            match item {
                MountItem::ScrollConfig {
                    id,
                    scrollbar_visible,
                    replaced,
                    ..
                } => {
                    scroll_nodes.push((*id, *scrollbar_visible, *replaced));
                }
                MountItem::Insert { parent, child, .. } => {
                    children_of.entry(*parent).or_default().push(*child);
                }
                MountItem::ViewProps { id, props } => {
                    pointer_events.insert(*id, props.pointer_events);
                }
                MountItem::Frame { id, frame } => {
                    frames.insert(*id, *frame);
                }
                _ => {}
            }
        }

        // First scroll() uses the built-in Canban scrollbar (platform indicator replaced & hidden).
        let (builtin_scroll_id, _, _) = scroll_nodes
            .iter()
            .copied()
            .find(|(_, visible, replaced)| !*visible && *replaced)
            .expect("built-in scrollbar replaces platform indicator on default scroll()");

        // Its decoration child covers the 300x120 viewport and MUST emit BoxNone on first mount
        // so clicks/drags inside scroll() pass straight through to content.
        let inserted = children_of
            .get(&builtin_scroll_id)
            .expect("scroll children");
        let decoration_host = *inserted
            .last()
            .expect("decoration scrollbar mounted last in scroll()");
        assert_eq!(
            frames.get(&decoration_host).map(|f| f.size),
            Some(Size {
                width: px(300.0),
                height: px(120.0),
            })
        );
        assert_eq!(
            pointer_events.get(&decoration_host),
            Some(&PointerEvents::BoxNone),
            "full-viewport scrollbar host must propagate BoxNone in MountItem::ViewProps on creation"
        );

        // Second scroll() opts out of built-in bar (`scrollbar(false)`) and pairs with an
        // outer-gutter `vertical_scrollbar` whose parent expands its hit-testable overflow.
        assert!(
            scroll_nodes
                .iter()
                .any(|(_, visible, replaced)| !*visible && !*replaced),
            "explicitly paired column scroll disables built-in indicator"
        );
        assert!(
            batch.items.iter().any(|item| matches!(
                item,
                MountItem::Overflow { id, extent: Some(ext) }
                    if *id != ROOT_ID && ext.origin.x.0 + ext.size.width.0 > 260.0
            )),
            "outer-gutter vertical_scrollbar extends parent hit-test overflow cleanly into column gap"
        );
    }

    #[test]
    fn thumb_fill_tracks_named_theme_changes() {
        #[component]
        fn ProbeRoot() -> impl IntoElement {
            crate::tokens::install_theme();
            let default_fill = thumb_fill(false).dark;
            crate::tokens::set_theme("gruvbox");
            let gruvbox_fill = thumb_fill(false).dark;
            assert_ne!(default_fill, gruvbox_fill);
            assert!((gruvbox_fill.a - 0.38).abs() < 0.01);
            assert!((thumb_fill(true).dark.a - 0.72).abs() < 0.01);
            view()
        }
        let mut core = Core::new(ApproxTextEngine, size(px(800.0), px(600.0)));
        core.mount_root(&nsh_core::element::Root::of::<ProbeRoot>());
        let _ = core.commit();
    }
}
