//! Small shared motion policy. NSH owns interpolation, clipping, hit testing and
//! modal focus. Product state never waits for a transition to finish.
use nsh_core::{
    element::{AnimationEnd, LayoutCurve, LayoutTransition},
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
        let generation = stored(0u64);
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
            let ticket = generation.get().wrapping_add(1);
            generation.replace(ticket);
            if !open && focused.get_untracked() {
                dismiss_keyboard();
            }
            if open {
                this.mounted.set(true);
            }
            if reduced {
                this.opacity.set(if open { 1. } else { 0. });
                this.mounted.set(open);
                return;
            }
            let ms = if open { PANEL_IN } else { PANEL_OUT };
            layout(this.region, ms);
            let end: AnimationEnd =
                this.opacity
                    .timing(if open { 1. } else { 0. }, ms, Easing::EaseOut);
            if std::env::var_os("CANBAN_MOTION_TRACE").is_some() {
                spawn(async move {
                    for _ in 0..24 {
                        next_frame().await;
                        if generation.get() != ticket {
                            break;
                        }
                        let value = this.opacity.value().await;
                        eprintln!("motion panel ticket={ticket} open={open} opacity={value:.3}");
                    }
                });
            }
            spawn(async move {
                if end.await && generation.get() == ticket && !open {
                    this.mounted.set(false);
                }
            });
        });
        this
    }
}
