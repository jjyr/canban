//! Small shared motion policy. NSH owns interpolation, clipping, hit testing and
//! modal focus. Product state never waits for a transition to finish.
use nsh_core::{
    element::{AnimationEnd, LayoutCurve, LayoutTransition, Spring},
    prelude::*,
};
pub const PANEL_IN: f32 = 220.;
pub const PANEL_OUT: f32 = 180.;
pub const COMPOSER: f32 = 160.;
/// An embedding host may opt into less motion; the OS preference always wins.
#[derive(Clone, Copy)]
pub struct ReducedMotion(pub Signal<bool>);
pub fn reduced() -> bool {
    reduce_motion() || use_context::<ReducedMotion>().is_some_and(|p| p.0.get())
}
/// NSH's native modal fade is 200ms; use its focus/dismissal lifecycle rather
/// than holding an application-owned invisible modal over the destination.
pub fn modal() -> ModalAnimation {
    if reduced() {
        ModalAnimation::None
    } else {
        ModalAnimation::Fade
    }
}
pub fn layout(region: ElementRef, ms: f32) {
    if !reduced() {
        region.animate_layout(LayoutTransition::new(LayoutCurve::EaseOut, ms));
    }
}
pub fn settle_spring() -> Spring {
    Spring::from_stiffness(1600.0).damping_ratio(1.0)
}
pub fn should_short_circuit_settle(current: Option<(f32, f32)>, to: f32) -> bool {
    current.is_some_and(|(cur, vel)| (to - cur).abs() < 0.5 && vel.abs() < 10.0)
}
pub fn shift(value: AnimatedValue, delta: f32) {
    if delta.abs() > 0.01 {
        value.set_offset(delta);
        value.flatten_offset();
    }
}
pub fn spring(value: AnimatedValue, to: f32, spec: Spring) -> Option<AnimationEnd> {
    if reduced() {
        value.set(to);
        None
    } else {
        Some(value.spring(to, spec))
    }
}
pub fn settle_from(
    value: AnimatedValue,
    current: Option<(f32, f32)>,
    to: f32,
    mut spec: Spring,
) -> Option<AnimationEnd> {
    if reduced() || should_short_circuit_settle(current, to) {
        value.set(to);
        None
    } else {
        if let Some((_, vel)) = current {
            spec.velocity = Some(vel);
        }
        Some(value.spring(to, spec))
    }
}
pub fn timing(value: AnimatedValue, to: f32, ms: f32, easing: Easing) -> Option<AnimationEnd> {
    if reduced() || ms <= 0.0 {
        value.set(to);
        None
    } else {
        Some(value.timing(to, ms, easing))
    }
}
pub fn when_done(
    ends: impl IntoIterator<Item = Option<AnimationEnd>>,
    on_complete: impl FnOnce() + 'static,
) {
    let ends: Vec<AnimationEnd> = ends.into_iter().flatten().collect();
    if ends.is_empty() {
        on_complete();
    } else {
        spawn(async move {
            let mut ok = true;
            for end in ends {
                ok &= end.await;
            }
            if ok {
                on_complete();
            }
        });
    }
}
/// Presence follows the requested state immediately; removal waits for a native
/// animation end. A new request cancels the old driver and fences its answer.
#[derive(Clone, Copy)]
pub struct Presence {
    pub region: ElementRef,
    pub mounted: Signal<bool>,
    pub opacity: AnimatedValue,
}
impl Presence {
    pub fn new(open: Memo<bool>) -> Self {
        let initial = open.get_untracked();
        let this = Self {
            region: element_ref(),
            mounted: signal(initial),
            opacity: animated_value(if initial { 1. } else { 0. }),
        };
        let previous = stored((initial, reduced()));
        let focused = signal(false);
        this.region.on_focus_in(move || focused.set(true));
        this.region.on_focus_out(move || focused.set(false));
        effect(move || {
            let open = open.get();
            let reduced = reduced();
            let changed = previous.replace((open, reduced)) != (open, reduced);
            if !changed {
                return;
            }
            if !open && focused.get_untracked() {
                dismiss_keyboard();
            }
            if open {
                this.mounted.set(true);
            }
            let ms = if open { PANEL_IN } else { PANEL_OUT };
            layout(this.region, ms);
            let end = timing(
                this.opacity,
                if open { 1. } else { 0. },
                ms,
                Easing::EaseOut,
            );
            if end.is_some() && std::env::var_os("CANBAN_MOTION_TRACE").is_some() {
                spawn(async move {
                    for _ in 0..24 {
                        next_frame().await;
                        if open != this.mounted.get_untracked() {
                            break;
                        }
                        let value = this.opacity.value().await;
                        eprintln!("motion panel open={open} opacity={value:.3}");
                    }
                });
            }
            when_done([end], move || {
                if !open {
                    this.mounted.set(false);
                }
            });
        });
        this
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::{
        ApproxTextEngine, Core, MountItem, NodeId,
        animation::{AnimationOp, Animator},
        geometry::size,
    };
    use std::cell::RefCell;

    thread_local! {
        static ACT: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    fn run_act(_: &()) {
        if let Some(f) = ACT.with(|slot| slot.borrow_mut().take()) {
            f();
        }
    }

    struct Harness {
        core: Core<ApproxTextEngine>,
        animator: Animator,
        trigger: NodeId,
        now: f64,
    }

    impl Harness {
        fn new<C: Component<Props = ()>>() -> Self {
            let mut core = Core::new(ApproxTextEngine, size(px(400.0), px(300.0)));
            core.mount_root(&nsh_core::element::Root::of::<C>());
            let mut this = Self {
                core,
                animator: Animator::default(),
                trigger: 0,
                now: 0.0,
            };
            this.flush();
            assert_ne!(
                this.trigger, 0,
                "Root must attach .test_id(\"act\").on_press(run_act)"
            );
            this
        }

        fn act<R: 'static>(&mut self, f: impl FnOnce() -> R + 'static) -> R {
            let out: std::rc::Rc<RefCell<Option<R>>> = std::rc::Rc::new(RefCell::new(None));
            let sink = out.clone();
            ACT.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move || {
                    *sink.borrow_mut() = Some(f());
                }));
            });
            assert!(
                self.core
                    .component_event(self.trigger, EventKind::Press, &EventPayload::None,)
            );
            self.flush();
            out.borrow_mut().take().expect("act closure ran")
        }

        fn flush(&mut self) {
            loop {
                self.core.run_component_tasks();
                let batch = self.core.commit();
                if let Some(batch) = &batch {
                    for item in &batch.items {
                        if let MountItem::ViewProps { id, props } = item
                            && props.test_id.as_deref() == Some("act")
                        {
                            self.trigger = *id;
                        }
                        self.animator.apply(item);
                    }
                }
                let had_finished = !self.animator.finished.is_empty();
                if had_finished {
                    self.core.animations_finished(&mut self.animator.finished);
                }
                if batch.is_none() && !had_finished {
                    break;
                }
            }
        }

        fn step_ms(&mut self, ms: f64) {
            self.now += ms / 1000.0;
            self.animator.tick(self.now);
            if !self.animator.finished.is_empty() {
                self.core.animations_finished(&mut self.animator.finished);
            }
            self.flush();
        }
    }

    #[test]
    fn shift_preserves_mid_spring_velocity_and_ignores_tiny_deltas() {
        thread_local! {
            static PROBE: RefCell<Option<Signal<u32>>> = const { RefCell::new(None) };
        }
        #[component]
        fn Root() -> impl IntoElement {
            let y = animated_value(0.0);
            let step = signal(0u32);
            effect(move || match step.get() {
                1 => {
                    let _ = spring(y, 120.0, settle_spring());
                }
                2 => {
                    shift(y, 0.005);
                }
                3 => {
                    shift(y, -40.0);
                    let _ = spring(y, 0.0, settle_spring());
                }
                _ => {}
            });
            PROBE.with(|p| *p.borrow_mut() = Some(step));
            view()
                .test_id("act")
                .on_press(run_act)
                .translate_y(px(0.0))
                .bind_translate_y(&y)
        }

        let mut h = Harness::new::<Root>();
        let step = PROBE.with(|p| p.borrow().unwrap());

        // Start spring 0 -> 120 and step two frames so velocity builds (> 0).
        h.act(move || step.set(1));
        h.step_ms(0.0);
        h.step_ms(16.0);

        let node_id = 1;
        let mid = h.animator.value(node_id);
        assert!(
            mid > 10.0 && mid < 110.0,
            "expected mid-spring position, got {mid}"
        );

        // Sub-centipoint shift is a no-op.
        h.act(move || step.set(2));
        assert!((h.animator.value(node_id) - mid).abs() < 1e-4);

        // Shift by -40 and retarget to 0: value immediately jumps by -40, and because positive
        // upward velocity was preserved rather than zeroed, the first frame after retargeting
        // reflects momentum instead of starting from rest!
        let mut baseline = Animator::default();
        baseline.apply(&MountItem::Animation(AnimationOp::CreateValue {
            node: 1,
            value: mid - 40.0,
        }));
        baseline.apply(&MountItem::Animation(AnimationOp::Start {
            node: 1,
            animation: 3,
            driver: nsh_core::animation::Driver::Spring {
                to: 0.0,
                stiffness: settle_spring().stiffness,
                damping: settle_spring().damping,
                mass: settle_spring().mass,
                velocity: Some(0.0),
                frame_rate: None,
            },
        }));
        baseline.tick(0.0);
        baseline.tick(0.016);
        let from_rest = baseline.value(1);

        h.act(move || step.set(3));
        assert!(
            (h.animator.value(node_id) - (mid - 40.0)).abs() < 1e-3,
            "shift must displace position immediately before next tick"
        );
        h.step_ms(0.0);
        h.step_ms(16.0);
        let with_momentum = h.animator.value(node_id);
        assert!(
            with_momentum > from_rest + 2.0,
            "expected positive velocity continuity ({with_momentum} vs at-rest {from_rest})"
        );
    }

    #[test]
    fn when_done_synchronously_completes_empty_and_cancels_on_retarget_or_set() {
        thread_local! {
            static EVENTS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
            static ACTION: RefCell<Option<Signal<u32>>> = const { RefCell::new(None) };
        }
        EVENTS.with(|e| e.borrow_mut().clear());

        #[component]
        fn Root() -> impl IntoElement {
            let x = animated_value(0.0);
            let y = animated_value(0.0);
            let action = signal(0u32);
            effect(move || match action.get() {
                0 => {
                    when_done([None], || {
                        EVENTS.with(|e| e.borrow_mut().push("sync_empty"));
                    });
                }
                1 => {
                    let ex = timing(x, 100.0, 100.0, Easing::Linear);
                    when_done([ex], || {
                        EVENTS.with(|e| e.borrow_mut().push("cancelled_by_retarget"));
                    });
                }
                2 => {
                    // Retarget `x` mid-flight: action 1's end resolves `false` and must NOT fire.
                    let ex = timing(x, 50.0, 100.0, Easing::Linear);
                    let ey = timing(y, 20.0, 200.0, Easing::Linear);
                    when_done([ex, ey], || {
                        EVENTS.with(|e| e.borrow_mut().push("both_completed"));
                    });
                }
                3 => {
                    let ex = timing(x, 0.0, 100.0, Easing::Linear);
                    when_done([ex], || {
                        EVENTS.with(|e| e.borrow_mut().push("cancelled_by_set"));
                    });
                }
                4 => {
                    x.set(0.0);
                }
                _ => {}
            });
            ACTION.with(|a| *a.borrow_mut() = Some(action));
            view()
                .test_id("act")
                .on_press(run_act)
                .bind_translate_x(&x)
                .bind_translate_y(&y)
        }

        let mut h = Harness::new::<Root>();
        assert_eq!(EVENTS.with(|e| e.borrow().clone()), vec!["sync_empty"]);

        let action = ACTION.with(|a| a.borrow().unwrap());
        h.act(move || action.set(1));
        h.step_ms(0.0);
        h.step_ms(40.0);

        // Retarget x while y starts a longer 200ms animation.
        h.act(move || action.set(2));
        h.step_ms(0.0);
        h.step_ms(120.0);
        // `x` finished at 100ms, `y` still running at 120ms: callback must wait for both!
        assert_eq!(EVENTS.with(|e| e.borrow().clone()), vec!["sync_empty"]);

        h.step_ms(100.0);
        assert_eq!(
            EVENTS.with(|e| e.borrow().clone()),
            vec!["sync_empty", "both_completed"]
        );

        // Start another animation and interrupt it via `set`.
        h.act(move || action.set(3));
        h.step_ms(0.0);
        h.step_ms(30.0);
        h.act(move || action.set(4));
        h.step_ms(200.0);
        assert_eq!(
            EVENTS.with(|e| e.borrow().clone()),
            vec!["sync_empty", "both_completed"]
        );
    }

    #[test]
    fn reduced_motion_and_settle_short_circuit_snap_without_spawning_drivers() {
        assert!(should_short_circuit_settle(Some((10.2, 4.0)), 10.0));
        assert!(!should_short_circuit_settle(Some((10.6, 4.0)), 10.0));
        assert!(!should_short_circuit_settle(Some((10.2, 12.0)), 10.0));
        assert!(!should_short_circuit_settle(None, 10.0));

        #[component]
        fn Root() -> impl IntoElement {
            let pref = signal(true);
            provide_context(ReducedMotion(pref));
            let v = animated_value(0.0);
            assert_eq!(modal(), ModalAnimation::None);
            assert!(spring(v, 50.0, settle_spring()).is_none());
            assert!(settle_from(v, Some((0.0, 500.0)), 80.0, settle_spring()).is_none());
            assert!(timing(v, 1.0, 200.0, Easing::EaseOut).is_none());

            pref.set(false);
            assert_eq!(modal(), ModalAnimation::Fade);
            assert!(settle_from(v, Some((99.7, 3.0)), 100.0, settle_spring()).is_none());
            let end = settle_from(v, Some((0.0, 250.0)), 100.0, settle_spring());
            assert!(end.is_some());
            drop(end);
            view().test_id("act").on_press(run_act).bind_opacity(&v)
        }

        let h = Harness::new::<Root>();
        assert!(h.animator.animating());
    }

    #[test]
    fn presence_mounts_fades_cancels_exit_on_reopen_and_snaps_on_reduced_motion() {
        type PresenceControls = (Signal<bool>, Signal<bool>, Presence);
        thread_local! {
            static CONTROLS: RefCell<Option<PresenceControls>> = const { RefCell::new(None) };
        }
        #[component]
        fn Root() -> impl IntoElement {
            let open_sig = signal(false);
            let reduced_sig = signal(false);
            provide_context(ReducedMotion(reduced_sig));
            let presence = Presence::new(computed(move || open_sig.get()));
            CONTROLS.with(|c| *c.borrow_mut() = Some((open_sig, reduced_sig, presence)));
            view()
                .test_id("act")
                .on_press(run_act)
                .element_ref(&presence.region)
                .bind_opacity(&presence.opacity)
        }

        let mut h = Harness::new::<Root>();
        let (open_sig, reduced_sig, presence) = CONTROLS.with(|c| c.borrow().unwrap());
        assert!(!h.act(move || presence.mounted.get_untracked()));
        assert_eq!(h.animator.value(1), 0.0);

        // Open: mounts immediately, fades to 1.0 over PANEL_IN.
        h.act(move || open_sig.set(true));
        assert!(h.act(move || presence.mounted.get_untracked()));
        h.step_ms(0.0);
        h.step_ms(250.0);
        assert!(h.act(move || presence.mounted.get_untracked()));
        assert!((h.animator.value(1) - 1.0).abs() < 1e-3);

        // Close, then re-open halfway through PANEL_OUT: exit end cancels (`false`), stays mounted!
        h.act(move || open_sig.set(false));
        h.step_ms(0.0);
        h.step_ms(90.0);
        assert!(h.act(move || presence.mounted.get_untracked()));

        h.act(move || open_sig.set(true));
        h.step_ms(0.0);
        h.step_ms(250.0);
        assert!(h.act(move || presence.mounted.get_untracked()));
        assert!((h.animator.value(1) - 1.0).abs() < 1e-3);

        // Close to completion: unmounts only after PANEL_OUT finishes.
        h.act(move || open_sig.set(false));
        h.step_ms(0.0);
        h.step_ms(100.0);
        assert!(h.act(move || presence.mounted.get_untracked()));
        h.step_ms(100.0);
        assert!(!h.act(move || presence.mounted.get_untracked()));
        assert!(h.animator.value(1).abs() < 1e-3);

        // Open again, start closing, then enable ReducedMotion mid-close -> snaps & unmounts synchronously!
        h.act(move || open_sig.set(true));
        h.step_ms(0.0);
        h.step_ms(250.0);
        h.act(move || open_sig.set(false));
        h.step_ms(0.0);
        h.step_ms(40.0);
        assert!(h.act(move || presence.mounted.get_untracked()));
        h.act(move || reduced_sig.set(true));
        assert!(!h.act(move || presence.mounted.get_untracked()));
        assert!(h.animator.value(1).abs() < 1e-3);
    }
}
