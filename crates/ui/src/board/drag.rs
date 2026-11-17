//! Board-scoped drag state. Native pan values drive the lifted Card directly;
//! the display-frame loop only hit-tests, changes slots and scrolls at edges.
use super::{
    Drop,
    model::{self, Layout, Slot},
};
use crate::{components::card_tile, presenter::*};
use canban_types::{Card, Project};
use canban_ui_kit::tokens::*;
use nsh_core::{element::AnimationEnd, keymap::When, prelude::*};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Default)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}
#[derive(Clone)]
struct Column {
    reference: ElementRef,
    scroll: ElementRef,
    rect: Rect,
    offset: f32,
}
#[derive(Default)]
struct Metrics {
    columns: BTreeMap<String, Column>,
    heights: BTreeMap<String, f32>,
    root: Rect,
    horizontal: f32,
    width: f32,
    dirty: bool,
    frames: u64,
    hit_micros: u128,
    layouts: u64,
}
#[derive(Clone)]
struct Session {
    card: Card,
    source: Rect,
    pointer: Point<Points>,
    target: Option<Slot>,
    original: Slot,
    original_y: f32,
    end: Option<PanEvent>,
}
#[derive(Clone, Copy)]
pub(super) struct BoardDrag {
    owner: Owner,
    ui: Ui,
    trace: bool,
    pub cards: Signal<Vec<Card>>,
    pub layout: Signal<Layout>,
    pub phase: Signal<u8>,
    pub lifted: Signal<String>,
    pub x: AnimatedValue,
    pub y: AnimatedValue,
    pub root: ElementRef,
    pub horizontal: ElementRef,
    project: Stored<Project>,
    metrics: Stored<Metrics>,
    session: Stored<Option<Session>>,
    pointer: Stored<Point<Points>>,
    pub ghost: Signal<Option<(Card, f32, f32, f32)>>,
}
type MotionEnd = (Option<AnimationEnd>, Option<AnimationEnd>);
pub(super) fn settle(value: AnimatedValue, to: f32) -> Option<AnimationEnd> {
    if reduce_motion() {
        value.set(to);
        None
    } else {
        Some(value.spring(
            to,
            Spring {
                stiffness: 1600.,
                damping: 80.,
                ..Default::default()
            },
        ))
    }
}
async fn wait_motion((x, y): MotionEnd) {
    if x.is_none() && y.is_none() {
        return;
    }
    let done = signal(false);
    spawn(async move {
        if let Some(end) = x {
            end.await;
        }
        if let Some(end) = y {
            end.await;
        }
        let _ = done.try_set(true);
    });
    let deadline = Instant::now() + Duration::from_millis(450);
    while done.try_get() == Some(false) && Instant::now() < deadline {
        timer(Duration::from_millis(16)).await;
    }
}
pub(super) fn can_start_drag(phase: u8, card_move_pending: bool) -> bool {
    phase == 0 && !card_move_pending
}
/// Whether NSH's pan gesture remains mounted on Board rows.
/// Keep `PanEnd` mounted during active gesture phases (`1` and `2`) so mouse release
/// delivers `on_pan_end`; disable during landing (`3`) or while `card.move` is in flight.
pub(super) fn is_row_pan_enabled(
    phase: u8,
    lifted_id: &str,
    row_id: &str,
    card_move_pending: bool,
) -> bool {
    if card_move_pending || phase == 3 {
        return false;
    }
    phase == 0 || lifted_id == row_id
}
pub(super) fn should_abort_unmounted_row(
    phase: Option<u8>,
    lifted_id: Option<&str>,
    unmounted_id: &str,
) -> bool {
    lifted_id == Some(unmounted_id) && matches!(phase, Some(1 | 2))
}
impl BoardDrag {
    pub fn new(ui: Ui, project: Project) -> Self {
        let this = Self {
            owner: Owner::current(),
            ui,
            trace: std::env::var_os("CANBAN_BOARD_TRACE").is_some(),
            cards: signal(Vec::new()),
            layout: signal(Layout::default()),
            phase: signal(0),
            lifted: signal(String::new()),
            x: animated_value(0.),
            y: animated_value(0.),
            root: element_ref(),
            horizontal: element_ref(),
            project: stored(project),
            metrics: stored(Metrics::default()),
            session: stored(None),
            pointer: stored(Point::default()),
            ghost: signal(None),
        };
        effect(move || {
            ui.snapshot.with(|_| ());
            ui.github.filters.with(|_| ());
            ui.github.gh_status.with(|_| ());
            ui.merging_cards.with(|_| ());
            ui.local_tags.with(|_| ());
            if this.phase.get_untracked() == 0 {
                this.sync();
            } else if this.trace {
                eprintln!("BOARD_DRAG_REFRESH_DEFERRED");
            }
        });
        effect(move || {
            let on_workspace = matches!(ui.page.get().as_str(), "board" | "card" | "new-card");
            if !on_workspace && this.phase.get_untracked() != 0 {
                this.end(PanEvent {
                    cancelled: true,
                    ..Default::default()
                });
            }
        });
        effect(move || {
            if this.phase.get() == 2 {
                on_key_with(
                    "escape",
                    KeyOptions {
                        when: When::Always,
                        ..Default::default()
                    },
                    move |_| {
                        this.end(PanEvent {
                            cancelled: true,
                            ..Default::default()
                        })
                    },
                );
            }
        });
        this
    }
    fn sync(self) {
        let project = self.project.with(|p| p.id.clone());
        self.cards.set(self.ui.snapshot.with_untracked(|s| {
            s.cards
                .iter()
                .filter(|c| {
                    c.project_id == project
                        && c.active()
                        && crate::github::filters::matches(self.ui, c, s)
                })
                .cloned()
                .collect()
        }));
        self.reflow();
    }
    pub fn height(self, id: &str, height: f32) {
        if height <= 0. {
            return;
        }
        let changed = self.metrics.with(|m| {
            if m.heights
                .get(id)
                .is_some_and(|old| (old - height).abs() < 0.5)
            {
                false
            } else {
                m.heights.insert(id.into(), height);
                true
            }
        });
        if changed {
            self.reflow();
        }
    }
    pub fn register(self, stage: &str, reference: ElementRef, scroll: ElementRef) {
        let position = self.ui.scrolls.with(|m| {
            m.get(&format!(
                "board:{}:{stage}",
                self.project.with(|p| p.id.clone())
            ))
            .copied()
            .unwrap_or_default()
        });
        self.metrics.with(|m| {
            m.columns.insert(
                stage.into(),
                Column {
                    reference,
                    scroll,
                    rect: Rect::default(),
                    offset: position.y.0,
                },
            );
            m.dirty = true;
        });
    }
    /// Forget unmounted columns (single-stage compact Board) so drag hit
    /// testing only targets visible stages.
    pub fn retain(self, stages: &[String]) {
        self.metrics.with(|m| {
            m.columns.retain(|stage, _| stages.contains(stage));
            m.dirty = true;
        });
    }
    pub fn dirty(self) {
        self.metrics.with(|m| m.dirty = true);
    }
    pub fn scrolled(self, stage: &str, offset: f32) {
        self.metrics.with(|m| {
            if let Some(c) = m.columns.get_mut(stage) {
                c.offset = offset;
            }
        });
    }
    pub fn horizontal_scroll(self, offset: f32) {
        self.metrics.with(|m| m.horizontal = offset);
    }
    pub fn content_width(self, width: f32) {
        self.metrics.with(|m| {
            m.width = width;
            m.dirty = true;
        });
    }
    fn reflow(self) {
        let stages = self.project.with(|p| {
            p.workflow
                .stages
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
        });
        let session = self.session.get();
        let layout = self.metrics.with(|m| {
            model::layout(
                &self.cards.get_untracked(),
                &stages,
                &m.heights,
                session.as_ref().map(|s| s.card.id.as_str()).unwrap_or(""),
                session.as_ref().and_then(|s| s.target.as_ref()),
            )
        });
        if self.layout.get_untracked() != layout {
            self.metrics.with(|m| m.layouts += 1);
            self.layout.set(layout);
        }
    }
    pub fn down(self, event: &PressEvent) {
        if self.phase.get_untracked() == 0 {
            self.pointer.replace(event.page_position);
            self.x.set(0.);
            self.y.set(0.);
        }
    }
    async fn measure(self) {
        if let Some(m) = self.root.measure().await {
            self.metrics.with(|g| {
                g.root = Rect {
                    x: m.page.x.0,
                    y: m.page.y.0,
                    w: m.frame.size.width.0,
                    h: m.frame.size.height.0,
                }
            });
        }
        let refs = self.metrics.with(|m| {
            m.columns
                .iter()
                .map(|(s, c)| (s.clone(), c.reference))
                .collect::<Vec<_>>()
        });
        for (stage, reference) in refs {
            if let Some(b) = reference.measure().await {
                self.metrics.with(|m| {
                    let horizontal = m.horizontal;
                    if let Some(c) = m.columns.get_mut(&stage) {
                        c.rect = Rect {
                            x: b.page.x.0 + horizontal,
                            y: b.page.y.0,
                            w: b.frame.size.width.0,
                            h: b.frame.size.height.0,
                        };
                    }
                });
            }
        }
        self.metrics.with(|m| m.dirty = false);
    }
    fn spawn_owned(self, future: impl std::future::Future<Output = ()> + 'static) {
        self.owner.run(|| {
            spawn(future);
        });
    }
    pub fn row_unmounted(self, id: &str) {
        if should_abort_unmounted_row(self.phase.try_get(), self.lifted.try_get().as_deref(), id) {
            self.clear();
        }
    }
    pub fn start(self, card: Card, reference: ElementRef) {
        if !can_start_drag(
            self.phase.get_untracked(),
            self.ui
                .pending_methods
                .with_untracked(|m| m.contains("card.move")),
        ) {
            return;
        }
        self.phase.set(1);
        self.metrics.with(|m| {
            m.frames = 0;
            m.hit_micros = 0;
            m.layouts = 0;
        });
        if self.trace {
            eprintln!("BOARD_DRAG_START card={}", card.id);
        }
        self.lifted.set(card.id.clone());
        let all = self.cards.get_untracked();
        let original = model::cards(&all, &card.stage, "")
            .iter()
            .position(|c| c.id == card.id)
            .unwrap_or(0);
        let original_y = *self
            .layout
            .get_untracked()
            .resting
            .get(&card.id)
            .unwrap_or(&0.);
        self.session.replace(Some(Session {
            original: Slot {
                stage: card.stage.clone(),
                index: original,
            },
            target: None,
            card,
            source: Rect::default(),
            pointer: self.pointer.get(),
            original_y,
            end: None,
        }));
        self.spawn_owned(async move {
            let Some(source) = reference.measure().await else {
                self.clear();
                return;
            };
            self.measure().await;
            if self.phase.try_get() != Some(1) {
                return;
            }
            let root = self.metrics.with(|m| m.root);
            let mut session = self.session.get().unwrap();
            // Rows are laid out at `top: resting_y` with `translate_y == 0` while idle,
            // so NSH `Measurement::page` already includes both `resting_y` and scrolling.
            session.source = Rect {
                x: source.page.x.0,
                y: source.page.y.0,
                w: source.frame.size.width.0,
                h: source.frame.size.height.0,
            };
            session.target = Some(session.original.clone());
            self.ghost.set(Some((
                session.card.clone(),
                source.page.x.0 - root.x,
                source.page.y.0 - root.y,
                source.frame.size.width.0,
            )));
            let end = session.end;
            self.session.replace(Some(session));
            self.reflow();
            self.phase.set(2);
            if self.trace {
                eprintln!("BOARD_DRAG_READY");
            }
            if let Some(end) = end {
                self.end(end);
                return;
            }
            let mut last = Instant::now();
            while self.phase.try_get() == Some(2) {
                next_frame().await;
                if self.phase.try_get() != Some(2) {
                    break;
                }
                let x = self.x.value().await;
                let y = self.y.value().await;
                if self.phase.try_get() != Some(2) {
                    break;
                }
                if self.metrics.with(|m| m.dirty) {
                    self.measure().await;
                }
                let dt = last.elapsed().as_secs_f32().min(0.04);
                last = Instant::now();
                self.track(x, y, dt);
            }
        });
    }
    fn track(self, x: f32, y: f32, dt: f32) {
        let began = Instant::now();
        let Some(mut session) = self.session.get() else {
            return;
        };
        let pointer = point(px(session.pointer.x.0 + x), px(session.pointer.y.0 + y));
        let (target, scrolls, horizontal) = self.metrics.with(|m| {
            let root = m.root;
            let px = pointer.x.0;
            let py = pointer.y.0;
            if px < root.x || px > root.x + root.w || py < root.y || py > root.y + root.h {
                return (None, Vec::new(), None);
            }
            let column = m
                .columns
                .iter()
                .filter(|(_, c)| py >= c.rect.y && py <= c.rect.y + c.rect.h)
                .min_by(|(_, a), (_, b)| {
                    ((a.rect.x - m.horizontal + a.rect.w / 2.) - px)
                        .abs()
                        .total_cmp(&((b.rect.x - m.horizontal + b.rect.w / 2.) - px).abs())
                });
            let mut scrolls = Vec::new();
            let target = column.map(|(stage, c)| {
                let mut offset = c.offset;
                let max = (self
                    .layout
                    .get_untracked()
                    .heights
                    .get(stage)
                    .copied()
                    .unwrap_or(0.)
                    - c.rect.h)
                    .max(0.);
                let next = (offset + model::edge_speed(py, c.rect.y, c.rect.h) * dt).clamp(0., max);
                if (next - offset).abs() > 0.1 {
                    scrolls.push((stage.clone(), c.scroll, next));
                    offset = next;
                }
                Slot {
                    stage: stage.clone(),
                    index: model::insertion(
                        &self.cards.get_untracked(),
                        stage,
                        &session.card.id,
                        &m.heights,
                        py - c.rect.y + offset,
                        session.target.as_ref(),
                    ),
                }
            });
            let next = (m.horizontal + model::edge_speed(px, root.x, root.w) * dt)
                .clamp(0., (m.width - root.w).max(0.));
            (
                target,
                scrolls,
                ((next - m.horizontal).abs() > 0.1).then_some(next),
            )
        });
        for (stage, reference, next) in scrolls {
            self.scrolled(&stage, next);
            reference.scroll_to(point(px(0.), px(next)), false);
        }
        if let Some(next) = horizontal {
            self.horizontal_scroll(next);
            self.horizontal.scroll_to(point(px(next), px(0.)), false);
        }
        if session.target != target {
            if self.trace {
                eprintln!("BOARD_DRAG_SLOT {:?}", target);
            }
            session.target = target;
            self.session.replace(Some(session));
            self.reflow();
        }
        self.metrics.with(|m| {
            m.frames += 1;
            m.hit_micros += began.elapsed().as_micros();
        });
    }
    pub fn end(self, event: PanEvent) {
        if self.phase.get_untracked() == 1 {
            self.session.with(|s| {
                if let Some(s) = s {
                    s.end = Some(event);
                }
            });
            return;
        }
        if self.phase.get_untracked() != 2 {
            return;
        }
        self.track(event.translation.x.0, event.translation.y.0, 0.);
        let Some(session) = self.session.get() else {
            return;
        };
        self.phase.set(3);
        if self.trace {
            self.metrics.with(|m| {
                eprintln!(
                    "BOARD_DRAG_END frames={} hit_micros={} layouts={} cancelled={}",
                    m.frames, m.hit_micros, m.layouts, event.cancelled
                )
            });
        }
        let target = if event.cancelled {
            None
        } else {
            session.target.clone()
        };
        let unchanged = target.as_ref() == Some(&session.original);
        if target.is_none() || unchanged {
            self.return_home();
            return;
        }
        let target = target.unwrap();
        let all = self.cards.get_untracked();
        let peers = model::cards(&all, &target.stage, &session.card.id);
        let before = peers.get(target.index).map(|c| c.id.clone());
        let intent = Drop {
            card_id: session.card.id.clone(),
            revision: session.card.revision,
            stage: target.stage,
            before_card_id: before,
        };
        let motion = self.land();
        let received = self.owner.run(|| signal(None::<bool>));
        if let Some(handler) = self.ui.board_move.get() {
            handler(intent);
            received.set(Some(true));
        } else {
            dispatch_complete(
                self.ui,
                "card.move",
                serde_json::json!({"card_id":intent.card_id,"revision":intent.revision,"stage":intent.stage,"before_card_id":intent.before_card_id}),
                move |result| {
                    received.try_set(Some(result.is_ok()));
                },
            );
        }
        self.spawn_owned(async move {
            wait_motion(motion).await;
            while received.try_get() == Some(None) {
                timer(Duration::from_millis(16)).await;
            }
            if self.phase.try_get() != Some(3) {
                return;
            }
            if received.try_get() == Some(Some(true)) {
                self.clear();
            } else {
                self.return_home();
            }
        });
    }
    fn land(self) -> MotionEnd {
        if let Some(s) = self.session.get()
            && let Some((stage, y, _)) = self.layout.get_untracked().gap
            && let Some(c) = self.metrics.with(|m| m.columns.get(&stage).cloned())
        {
            let horizontal = self.metrics.with(|m| m.horizontal);
            return (
                settle(self.x, c.rect.x - horizontal - s.source.x),
                settle(self.y, c.rect.y - c.offset + y - s.source.y),
            );
        }
        (None, None)
    }
    fn return_home(self) {
        self.session.with(|s| {
            if let Some(s) = s {
                s.target = Some(s.original.clone());
            }
        });
        self.reflow();
        let motion = if let Some(s) = self.session.get()
            && let Some(c) = self
                .metrics
                .with(|m| m.columns.get(&s.original.stage).cloned())
        {
            let horizontal = self.metrics.with(|m| m.horizontal);
            (
                settle(self.x, c.rect.x - horizontal - s.source.x),
                settle(self.y, c.rect.y - c.offset + s.original_y - s.source.y),
            )
        } else {
            (None, None)
        };
        self.spawn_owned(async move {
            wait_motion(motion).await;
            if self.phase.try_get().is_some() {
                self.clear();
            }
        });
    }
    fn clear(self) {
        if self.phase.try_get().is_none() {
            return;
        }
        self.phase.set(0);
        self.session.replace(None);
        self.ghost.set(None);
        self.lifted.set(String::new());
        self.x.set(0.);
        self.y.set(0.);
        self.sync();
    }
    pub fn overlay(self) -> impl IntoElement {
        dynamic(move || match self.ghost.get() {
            None => view().into_element(),
            Some((card, x, y, width)) => view()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(width))
                .z_index(100)
                .pointer_events(PointerEvents::None)
                .shadow_lg()
                .rounded(px(RADIUS))
                .bind_translate_x(&self.x)
                .bind_translate_y(&self.y)
                .child(card_tile(self.ui, card))
                .into_element(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_eligibility_scopes_pending_check_to_card_move_and_idle_phase() {
        assert!(can_start_drag(0, false));
        assert!(!can_start_drag(0, true));
        for active_phase in [1, 2, 3] {
            assert!(!can_start_drag(active_phase, false));
            assert!(!can_start_drag(active_phase, true));
        }
        // The lifted row MUST keep pan_enabled during phases 1 and 2 so NSH preserves EventKind::PanEnd!
        assert!(is_row_pan_enabled(0, "", "c1", false));
        assert!(!is_row_pan_enabled(0, "", "c1", true));
        assert!(is_row_pan_enabled(1, "c1", "c1", false));
        assert!(is_row_pan_enabled(2, "c1", "c1", false));
        assert!(!is_row_pan_enabled(1, "c1", "c2", false));
        assert!(!is_row_pan_enabled(2, "c1", "c2", false));
        assert!(!is_row_pan_enabled(3, "c1", "c1", false));
    }

    #[test]
    fn unmounted_dragged_row_aborts_active_gesture_without_interrupting_landing_or_peers() {
        assert!(should_abort_unmounted_row(Some(1), Some("c1"), "c1"));
        assert!(should_abort_unmounted_row(Some(2), Some("c1"), "c1"));
        // Do not clear during phase 3 (landing / awaiting card.move completion) or for peer rows.
        assert!(!should_abort_unmounted_row(Some(3), Some("c1"), "c1"));
        assert!(!should_abort_unmounted_row(Some(0), Some("c1"), "c1"));
        assert!(!should_abort_unmounted_row(Some(2), Some("c1"), "c2"));
        assert!(!should_abort_unmounted_row(None, Some("c1"), "c1"));
    }
}
