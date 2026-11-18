//! Board-scoped drag state. Native pan values drive the lifted Card directly;
//! the display-frame loop only hit-tests, changes slots and scrolls at edges.
use super::{
    Drop,
    model::{self, Layout, PendingMove, Slot},
    transitions::{self, ColumnMetric, EntryKind, ExitSpec, FlightSpec},
};
use crate::{components::card_tile, presenter::*};
use canban_types::{BoardFilter, Card, Project};
use canban_ui_kit::{motion, tokens::*};
use nsh_core::{keymap::When, prelude::*};
use std::{collections::BTreeMap, time::Instant};
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
    pan_x: f32,
    pan_y: f32,
    target: Option<Slot>,
    original: Slot,
    end: Option<PanEvent>,
}
#[derive(Clone)]
struct ReleaseOrigin {
    card_id: String,
    origin_x: f32,
    origin_y: f32,
    vel_x: f32,
    vel_y: f32,
}
#[derive(Clone, Copy)]
pub(super) struct BoardDrag {
    owner: Owner,
    ui: Ui,
    trace: bool,
    compact: Memo<bool>,
    initialized: Stored<bool>,
    last_filter: Stored<BoardFilter>,
    pending_move: Stored<Option<PendingMove>>,
    pub cards: Signal<Vec<Card>>,
    pub layout: Signal<Layout>,
    viewports: Signal<BTreeMap<String, (f32, f32)>>,
    reflow_pending: Stored<bool>,
    heights_settled: Stored<bool>,
    measure_pending: Stored<bool>,
    order: Stored<model::Order>,
    transition_gen: Stored<u64>,
    pub motion_epoch: Signal<u64>,
    entries: Stored<(u64, BTreeMap<String, EntryKind>)>,
    pub flights: Signal<Vec<FlightSpec>>,
    pub exits: Signal<Vec<ExitSpec>>,
    pub focused: Signal<String>,
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
#[derive(Default)]
struct HitTest {
    target: Option<Slot>,
    scrolls: Vec<(String, ElementRef, f32)>,
    horizontal: Option<f32>,
}
pub(super) fn can_start_drag(phase: u8, card_move_pending: bool) -> bool {
    phase == 0 && !card_move_pending
}
/// Whether NSH's pan gesture remains mounted on Board rows.
/// Keep `PanEnd` mounted during active gesture phases (`1` and `2`) so mouse release
/// delivers `on_pan_end`; disable while `card.move` is in flight.
pub(super) fn is_row_pan_enabled(
    phase: u8,
    lifted_id: &str,
    row_id: &str,
    card_move_pending: bool,
) -> bool {
    if card_move_pending || phase > 2 {
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
    pub fn new(ui: Ui, project: Project, compact: Memo<bool>) -> Self {
        let initial_filter = ui
            .github
            .filters
            .with_untracked(|m| m.get(&project.id).cloned().unwrap_or_default());
        let this = Self {
            owner: Owner::current(),
            ui,
            trace: std::env::var_os("CANBAN_BOARD_TRACE").is_some(),
            compact,
            initialized: stored(false),
            last_filter: stored(initial_filter),
            pending_move: stored(None),
            cards: signal(Vec::new()),
            layout: signal(Layout::default()),
            viewports: signal(BTreeMap::new()),
            reflow_pending: stored(false),
            heights_settled: stored(false),
            measure_pending: stored(false),
            order: stored(model::Order::new()),
            transition_gen: stored(0),
            motion_epoch: signal(0),
            entries: stored((0, BTreeMap::new())),
            flights: signal(Vec::new()),
            exits: signal(Vec::new()),
            focused: signal(String::new()),
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
        let prev_env = stored(None::<(bool, bool, bool)>);
        effect(move || {
            let reduced = motion::reduced();
            let visible = this.board_visible_reactive();
            let is_compact = compact.get();
            let Some((was_reduced, was_visible, was_compact)) =
                prev_env.replace(Some((reduced, visible, is_compact)))
            else {
                return;
            };
            let became_reduced = reduced && !was_reduced;
            let became_hidden = !visible && was_visible;
            let compact_changed = is_compact != was_compact;
            if became_reduced || became_hidden || compact_changed {
                batch(|| {
                    if this.phase.get_untracked() != 0 {
                        this.clear();
                    }
                    this.clear_transitions();
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
    fn board_visible_reactive(self) -> bool {
        let page = self.ui.page.get();
        let on_board = matches!(page.as_str(), "board" | "card" | "new-card");
        let full = self.ui.narrow.get()
            || self.ui.card_standalone.get()
            || (page == "card" && self.ui.card_terminal_open.get());
        on_board && !full
    }
    fn board_visible(self) -> bool {
        untracked(|| self.board_visible_reactive())
    }
    fn column_metrics(self) -> (f32, BTreeMap<String, ColumnMetric>) {
        self.metrics.with(|m| {
            let cols = m
                .columns
                .iter()
                .map(|(stage, c)| {
                    (
                        stage.clone(),
                        ColumnMetric {
                            x: c.rect.x - m.horizontal - m.root.x,
                            y: c.rect.y - m.root.y,
                            w: c.rect.w,
                            h: c.rect.h,
                            offset: c.offset,
                        },
                    )
                })
                .collect();
            (m.root.w, cols)
        })
    }
    fn clear_transitions(self) {
        self.entries.with(|(_, map)| map.clear());
        self.motion_epoch.update(|e| *e = e.wrapping_add(1));
        if !self.flights.with_untracked(Vec::is_empty) {
            self.flights.set(Vec::new());
        }
        if !self.exits.with_untracked(Vec::is_empty) {
            self.exits.set(Vec::new());
        }
    }
    pub fn cancel_transitions(self, id: &str) {
        self.entries.with(|(_, map)| {
            map.remove(id);
        });
        if self
            .flights
            .with_untracked(|all| all.iter().any(|f| f.id == id))
        {
            self.flights.update(|all| all.retain(|f| f.id != id));
        }
    }
    pub fn take_entry(self, id: &str) -> Option<EntryKind> {
        self.entries.with(|(_, map)| map.remove(id))
    }
    fn finish_flight(self, id: &str, generation: u64) {
        if self
            .flights
            .try_get()
            .is_some_and(|all| all.iter().any(|f| f.id == id && f.generation == generation))
        {
            self.flights
                .update(|all| all.retain(|f| !(f.id == id && f.generation == generation)));
        }
    }
    fn finish_exit(self, id: &str, generation: u64) {
        if self
            .exits
            .try_get()
            .is_some_and(|all| all.iter().any(|e| e.id == id && e.generation == generation))
        {
            self.exits
                .update(|all| all.retain(|e| !(e.id == id && e.generation == generation)));
        }
    }
    fn make_release_flight(
        self,
        release: &ReleaseOrigin,
        new_cards: &[Card],
        new_layout: &Layout,
        cols: &BTreeMap<String, ColumnMetric>,
    ) -> Option<FlightSpec> {
        if untracked(motion::reduced) || !self.board_visible() || self.compact.get_untracked() {
            return None;
        }
        let card = new_cards.iter().find(|c| c.id == release.card_id)?;
        let nc = cols.get(&card.stage)?;
        if nc.w <= 0.0 || nc.h <= 0.0 {
            return None;
        }
        let ny = new_layout.resting.get(&card.id).copied()?;
        let target_x = nc.x;
        let target_y = nc.y + (ny - nc.offset);
        if motion::should_short_circuit_settle(
            Some((release.origin_x - target_x, release.vel_x)),
            0.0,
        ) && motion::should_short_circuit_settle(
            Some((release.origin_y - target_y, release.vel_y)),
            0.0,
        ) {
            return None;
        }
        let generation = self.transition_gen.with(|next_gen| {
            *next_gen = next_gen.wrapping_add(1);
            *next_gen
        });
        Some(FlightSpec {
            id: card.id.clone(),
            card: card.clone(),
            origin_x: release.origin_x,
            origin_y: release.origin_y,
            target_x,
            target_y,
            width: nc.w,
            from_opacity: 1.0,
            to_opacity: 1.0,
            target_stage: card.stage.clone(),
            exiting: false,
            initial_velocity: Some((release.vel_x, release.vel_y)),
            generation,
        })
    }
    fn sync(self) {
        self.sync_with(true);
    }
    fn sync_with(self, animate: bool) {
        self.sync_internal(animate, None);
    }
    fn sync_internal(self, animate: bool, release: Option<ReleaseOrigin>) {
        let project = self.project.with(|p| p.id.clone());
        let current_filter = self
            .ui
            .github
            .filters
            .with_untracked(|m| m.get(&project).cloned().unwrap_or_default());
        let started = Instant::now();
        let changed = self.ui.snapshot.with_untracked(|s| {
            if let Some(pending) = self.pending_move.get() {
                let clear_pending = match s.cards.iter().find(|c| c.id == pending.card_id) {
                    None => true,
                    Some(c) => !c.active() || c.revision > pending.base_revision,
                };
                if clear_pending {
                    self.pending_move.replace(None);
                }
            }
            let mut new_cards: Vec<Card> = s
                .cards
                .iter()
                .filter(|c| {
                    c.project_id == project
                        && c.active()
                        && crate::github::filters::matches(self.ui, c)
                })
                .cloned()
                .collect();
            if let Some(pending) = self.pending_move.get() {
                model::apply_pending_move(&mut new_cards, &pending);
            }
            let was_initialized = self.initialized.get();
            let cards_unchanged =
                was_initialized && self.cards.with_untracked(|old| old == &new_cards);
            if cards_unchanged && release.is_none() {
                return false;
            }
            self.initialized.replace(true);
            let filter_changed = self.last_filter.replace(current_filter.clone()) != current_filter;
            let can_animate = animate
                && was_initialized
                && !filter_changed
                && self.board_visible()
                && !untracked(motion::reduced);
            let new_order = model::order(&new_cards);
            let stages = self.project.with(|p| {
                p.workflow
                    .stages
                    .iter()
                    .map(|s| s.id.clone())
                    .collect::<Vec<_>>()
            });
            let session = self.session.get();
            let new_layout = self.metrics.with(|m| {
                model::layout_ordered(
                    &new_cards,
                    &stages,
                    &new_order,
                    &m.heights,
                    session.as_ref().map(|s| s.card.id.as_str()).unwrap_or(""),
                    session.as_ref().and_then(|s| s.target.as_ref()),
                )
            });

            batch(|| {
                if cards_unchanged {
                    let (_, cols) = self.column_metrics();
                    if let Some(rel) = release.as_ref() {
                        if let Some(spec) =
                            self.make_release_flight(rel, &new_cards, &new_layout, &cols)
                        {
                            self.flights.update(|all| {
                                all.retain(|f| f.id != spec.id);
                                all.push(spec);
                            });
                        } else if self
                            .flights
                            .with_untracked(|all| all.iter().any(|f| f.id == rel.card_id))
                        {
                            self.flights
                                .update(|all| all.retain(|f| f.id != rel.card_id));
                        }
                    }
                    self.order.replace(new_order);
                    if self.layout.with_untracked(|old| old != &new_layout) {
                        self.metrics.with(|m| m.layouts += 1);
                        self.layout.set(new_layout);
                    }
                    return;
                }

                if can_animate {
                    let (root_w, cols) = self.column_metrics();
                    let active_flights = self.flights.get_untracked();
                    let exclude_id = release.as_ref().map(|r| r.card_id.as_str());
                    let mut plan = self.cards.with_untracked(|old_cards| {
                        self.layout.with_untracked(|old_layout| {
                            self.metrics.with(|m| {
                                self.transition_gen.with(|next_gen| {
                                    transitions::plan(
                                        old_cards,
                                        &new_cards,
                                        old_layout,
                                        &new_layout,
                                        &m.heights,
                                        &cols,
                                        root_w,
                                        &active_flights,
                                        exclude_id,
                                        next_gen,
                                    )
                                })
                            })
                        })
                    });
                    if let Some(rel) = release.as_ref()
                        && let Some(spec) =
                            self.make_release_flight(rel, &new_cards, &new_layout, &cols)
                    {
                        plan.flights.retain(|f| f.id != spec.id);
                        plan.flights.push(spec);
                    }
                    if plan.flights != active_flights {
                        self.flights.set(plan.flights);
                    }
                    if !plan.exits.is_empty()
                        || self.exits.with_untracked(|exits| {
                            exits.iter().any(|e| new_cards.iter().any(|c| c.id == e.id))
                        })
                    {
                        self.exits.update(|exits| {
                            exits.retain(|e| {
                                !new_cards.iter().any(|c| c.id == e.id)
                                    && !plan.exits.iter().any(|n| n.id == e.id)
                            });
                            exits.extend(plan.exits);
                        });
                    }
                    let entry_seq = self.entries.with(|(seq, map)| {
                        *seq = seq.wrapping_add(1);
                        *map = plan.entries;
                        *seq
                    });
                    if self.entries.with(|(_, map)| !map.is_empty()) {
                        self.spawn_owned(async move {
                            next_frame().await;
                            next_frame().await;
                            if self.phase.try_get().is_some() {
                                self.entries.with(|(seq, map)| {
                                    if *seq == entry_seq {
                                        map.clear();
                                    }
                                });
                            }
                        });
                    }
                } else {
                    self.clear_transitions();
                }

                self.order.replace(new_order);
                self.cards.set(new_cards);
                if self.layout.with_untracked(|old| old != &new_layout) {
                    self.metrics.with(|m| m.layouts += 1);
                    self.layout.set(new_layout);
                }
            });
            true
        });
        if self.trace {
            eprintln!(
                "BOARD_SYNC cards={} changed={changed} micros={}",
                self.cards.with_untracked(Vec::len),
                started.elapsed().as_micros()
            );
        }
    }
    /// A single frame task absorbs all native height callbacks from this commit.
    fn schedule_reflow(self) {
        if self.reflow_pending.replace(true) {
            return;
        }
        self.spawn_owned(async move {
            next_frame().await;
            self.reflow_pending.replace(false);
            batch(|| {
                if !self.heights_settled.replace(true) {
                    self.motion_epoch.update(|e| *e = e.wrapping_add(1));
                }
                self.reflow();
            });
        });
    }
    fn schedule_measure(self) {
        if self.measure_pending.replace(true) {
            return;
        }
        self.spawn_owned(async move {
            next_frame().await;
            self.measure_pending.replace(false);
            if self.phase.try_get() == Some(0) && self.metrics.with(|m| m.dirty) {
                self.measure().await;
            }
        });
    }
    pub fn is_ghost_active(self, id: &str) -> bool {
        self.ghost
            .with(|g| g.as_ref().is_some_and(|(c, ..)| c.id == id))
            || self
                .flights
                .with(|all| all.iter().any(|f| !f.exiting && f.id == id))
    }
    pub fn visible(self, stage: &str) -> Vec<Card> {
        let (offset, height) = self
            .viewports
            .with(|v| v.get(stage).copied().unwrap_or((0., 720.)));
        let lifted = self.lifted.get();
        let focused = self.focused.get();
        self.cards.with(|cards| {
            self.layout.with(|layout| {
                let rows = layout.columns.get(stage).map(Vec::as_slice).unwrap_or(&[]);
                let mut indices: Vec<_> = model::visible(rows, offset, height)
                    .iter()
                    .map(|r| r.index)
                    .collect();
                // Never dispose the native gesture/focus owner merely because it scrolls away.
                for id in [&lifted, &focused].into_iter().filter(|id| !id.is_empty()) {
                    if let Some(i) = cards.iter().position(|c| &c.id == id && c.stage == stage)
                        && !indices.contains(&i)
                    {
                        indices.push(i);
                    }
                }
                indices
                    .into_iter()
                    .filter_map(|i| cards.get(i).cloned())
                    .collect()
            })
        })
    }
    pub fn viewport(self, stage: &str, height: f32) {
        self.dirty();
        let old = self
            .viewports
            .with_untracked(|v| v.get(stage).copied().unwrap_or((0., 720.)));
        if (old.1 - height).abs() > 0.5 {
            self.viewports.update(|v| {
                v.insert(stage.into(), (old.0, height));
            });
        }
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
            self.schedule_reflow();
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
        self.viewports.update(|v| {
            v.insert(stage.into(), (position.y.0, 720.));
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
        });
        self.dirty();
    }
    /// Forget unmounted columns (single-stage compact Board) so drag hit
    /// testing only targets visible stages.
    pub fn retain(self, stages: &[String]) {
        self.metrics.with(|m| {
            m.columns.retain(|stage, _| stages.contains(stage));
        });
        self.dirty();
    }
    pub fn dirty(self) {
        self.metrics.with(|m| m.dirty = true);
        if self.phase.try_get() == Some(0) {
            self.schedule_measure();
        }
    }
    pub fn scrolled(self, stage: &str, offset: f32) {
        if self
            .viewports
            .with_untracked(|v| v.get(stage).is_none_or(|p| (p.0 - offset).abs() > 0.1))
        {
            self.viewports.update(|v| {
                let p = v.entry(stage.into()).or_insert((0., 720.));
                p.0 = offset;
            });
        }
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
        });
        self.dirty();
    }
    fn reflow(self) {
        let started = Instant::now();
        let stages = self.project.with(|p| {
            p.workflow
                .stages
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
        });
        let session = self.session.get();
        let layout = self.metrics.with(|m| {
            self.cards.with_untracked(|cards| {
                self.order.with(|order| {
                    model::layout_ordered(
                        cards,
                        &stages,
                        order,
                        &m.heights,
                        session.as_ref().map(|s| s.card.id.as_str()).unwrap_or(""),
                        session.as_ref().and_then(|s| s.target.as_ref()),
                    )
                })
            })
        });
        if self.layout.with_untracked(|old| old != &layout) {
            self.metrics.with(|m| m.layouts += 1);
            self.layout.set(layout);
            if self.trace {
                eprintln!("BOARD_REFLOW micros={}", started.elapsed().as_micros());
            }
        }
    }
    pub fn down(self, event: &PressEvent) {
        if self.phase.get_untracked() == 0 {
            self.pointer.replace(event.page_position);
            self.x.set(0.);
            self.y.set(0.);
        }
    }
    /// Issue all layout measurements concurrently so NSH answers them in a single commit.
    async fn measure_all(self, source_ref: Option<ElementRef>) -> Option<Measurement> {
        let source_answer = source_ref.map(|r| r.measure());
        let root_answer = self.root.measure();
        let refs = self.metrics.with(|m| {
            m.columns
                .iter()
                .map(|(s, c)| (s.clone(), c.reference))
                .collect::<Vec<_>>()
        });
        let column_answers: Vec<_> = refs
            .into_iter()
            .map(|(stage, reference)| (stage, reference.measure()))
            .collect();
        let source = match source_answer {
            Some(answer) => answer.await,
            None => None,
        };
        if let Some(m) = root_answer.await {
            self.metrics.with(|g| {
                g.root = Rect {
                    x: m.page.x.0,
                    y: m.page.y.0,
                    w: m.frame.size.width.0,
                    h: m.frame.size.height.0,
                }
            });
        }
        for (stage, answer) in column_answers {
            if let Some(b) = answer.await {
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
        source
    }
    async fn measure(self) {
        let _ = self.measure_all(None).await;
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
    pub fn start(self, card: Card, reference: ElementRef, event: &PanEvent) {
        if !can_start_drag(
            self.phase.get_untracked(),
            self.ui
                .pending_methods
                .with_untracked(|m| m.contains("card.move")),
        ) {
            return;
        }
        self.cancel_transitions(&card.id);
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
        let original = self.cards.with_untracked(|all| {
            model::cards(all, &card.stage, "")
                .iter()
                .position(|c| c.id == card.id)
                .unwrap_or(0)
        });
        let pan_x = event.value_x.unwrap_or(event.translation.x.0);
        let pan_y = event.value_y.unwrap_or(event.translation.y.0);
        self.session.replace(Some(Session {
            original: Slot {
                stage: card.stage.clone(),
                index: original,
            },
            target: None,
            card,
            source: Rect::default(),
            pointer: self.pointer.get(),
            pan_x,
            pan_y,
            end: None,
        }));
        self.spawn_owned(async move {
            let Some(source) = self.measure_all(Some(reference)).await else {
                self.clear();
                return;
            };
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
            let end = session.end.take();
            if let Some(end) = end {
                let hit = self.hit_test(&session, end.translation.x.0, end.translation.y.0, 0.);
                if end.cancelled
                    || hit.target.is_none()
                    || hit.target.as_ref() == Some(&session.original)
                {
                    self.clear();
                    return;
                }
            }
            self.ghost.set(Some((
                session.card.clone(),
                source.page.x.0 - root.x,
                source.page.y.0 - root.y,
                source.frame.size.width.0,
            )));
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
    fn hit_test(self, session: &Session, x: f32, y: f32, dt: f32) -> HitTest {
        let pointer = point(px(session.pointer.x.0 + x), px(session.pointer.y.0 + y));
        self.metrics.with(|m| {
            let root = m.root;
            let px = pointer.x.0;
            let py = pointer.y.0;
            if px < root.x || px > root.x + root.w || py < root.y || py > root.y + root.h {
                return HitTest::default();
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
                let max = self.layout.with_untracked(|l| {
                    (l.heights.get(stage).copied().unwrap_or(0.) - c.rect.h).max(0.)
                });
                let next = (offset + model::edge_speed(py, c.rect.y, c.rect.h) * dt).clamp(0., max);
                if (next - offset).abs() > 0.1 {
                    scrolls.push((stage.clone(), c.scroll, next));
                    offset = next;
                }
                Slot {
                    stage: stage.clone(),
                    index: self.cards.with_untracked(|cards| {
                        model::insertion(
                            cards,
                            stage,
                            &session.card.id,
                            &m.heights,
                            py - c.rect.y + offset,
                            session.target.as_ref(),
                        )
                    }),
                }
            });
            let next = (m.horizontal + model::edge_speed(px, root.x, root.w) * dt)
                .clamp(0., (m.width - root.w).max(0.));
            HitTest {
                target,
                scrolls,
                horizontal: ((next - m.horizontal).abs() > 0.1).then_some(next),
            }
        })
    }
    fn track(self, x: f32, y: f32, dt: f32) {
        let began = Instant::now();
        let Some(mut session) = self.session.get() else {
            return;
        };
        session.pan_x = x;
        session.pan_y = y;
        let hit = self.hit_test(&session, x, y, dt);
        for (stage, reference, next) in hit.scrolls {
            self.scrolled(&stage, next);
            reference.scroll_to(point(px(0.), px(next)), false);
        }
        if let Some(next) = hit.horizontal {
            self.horizontal_scroll(next);
            self.horizontal.scroll_to(point(px(next), px(0.)), false);
        }
        let target_changed = session.target != hit.target;
        if target_changed {
            if self.trace {
                eprintln!("BOARD_DRAG_SLOT {:?}", hit.target);
            }
            session.target = hit.target;
        }
        self.session.replace(Some(session));
        if target_changed {
            self.reflow();
        }
        self.metrics.with(|m| {
            m.frames += 1;
            m.hit_micros += began.elapsed().as_micros();
        });
    }
    pub fn end(self, event: PanEvent) {
        if self.phase.get_untracked() == 1 {
            if event.cancelled {
                self.clear();
                return;
            }
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
        if !event.cancelled {
            self.track(event.translation.x.0, event.translation.y.0, 0.);
        }
        let Some(session) = self.session.get() else {
            return;
        };
        if self.trace {
            self.metrics.with(|m| {
                eprintln!(
                    "BOARD_DRAG_END frames={} hit_micros={} layouts={} cancelled={}",
                    m.frames, m.hit_micros, m.layouts, event.cancelled
                )
            });
        }
        let has_event_pos = event.value_x.is_some()
            || event.value_y.is_some()
            || event.translation.x.0 != 0.0
            || event.translation.y.0 != 0.0;
        let val_x = event.value_x.unwrap_or(if has_event_pos {
            event.translation.x.0
        } else {
            session.pan_x
        });
        let val_y = event.value_y.unwrap_or(if has_event_pos {
            event.translation.y.0
        } else {
            session.pan_y
        });
        let (origin_x, origin_y) = self.ghost.with_untracked(|g| {
            g.as_ref()
                .map(|(_, gx, gy, _)| (gx + val_x, gy + val_y))
                .unwrap_or_else(|| {
                    let root = self.metrics.with(|m| m.root);
                    (
                        session.source.x - root.x + val_x,
                        session.source.y - root.y + val_y,
                    )
                })
        });
        let release = ReleaseOrigin {
            card_id: session.card.id.clone(),
            origin_x,
            origin_y,
            vel_x: event.velocity_x,
            vel_y: event.velocity_y,
        };
        let target = if event.cancelled {
            None
        } else {
            session.target.clone()
        };
        let unchanged = target.is_none() || target.as_ref() == Some(&session.original);
        if unchanged {
            batch(|| {
                self.session.replace(None);
                self.ghost.set(None);
                self.lifted.set(String::new());
                self.x.set(0.);
                self.y.set(0.);
                self.phase.set(0);
                self.sync_internal(true, Some(release));
            });
            return;
        }
        let target = target.unwrap();
        let before = self.cards.with_untracked(|all| {
            model::cards(all, &target.stage, &session.card.id)
                .get(target.index)
                .map(|c| c.id.clone())
        });
        let intent = Drop {
            card_id: session.card.id.clone(),
            revision: session.card.revision,
            stage: target.stage.clone(),
            before_card_id: before.clone(),
        };
        let card_id = intent.card_id.clone();
        let base_revision = intent.revision;
        let on_complete = move |result: Result<serde_json::Value, String>| {
            if self.phase.try_get().is_none() {
                return;
            }
            let matched = self.pending_move.with(|pm| {
                if pm
                    .as_ref()
                    .is_some_and(|p| p.card_id == card_id && p.base_revision == base_revision)
                {
                    *pm = None;
                    true
                } else {
                    false
                }
            });
            if result.is_err() && matched {
                self.sync_with(true);
            }
        };
        if let Some(handler) = self.ui.board_move.get() {
            batch(|| {
                self.session.replace(None);
                self.ghost.set(None);
                self.lifted.set(String::new());
                self.x.set(0.);
                self.y.set(0.);
                self.phase.set(0);
                handler(intent);
                self.sync_internal(true, Some(release));
            });
        } else {
            self.pending_move.replace(Some(PendingMove {
                card_id: intent.card_id.clone(),
                base_revision: intent.revision,
                target_stage: target.stage,
                before_card_id: before,
            }));
            batch(|| {
                self.session.replace(None);
                self.ghost.set(None);
                self.lifted.set(String::new());
                self.x.set(0.);
                self.y.set(0.);
                self.phase.set(0);
                self.sync_internal(true, Some(release));
            });
            self.dispatch_move(intent, on_complete);
        }
    }
    fn dispatch_move(
        self,
        intent: Drop,
        after: impl FnOnce(Result<serde_json::Value, String>) + 'static,
    ) {
        #[cfg(test)]
        if let Some(mock) = TEST_MOVE_RPC.with(|m| m.borrow().clone()) {
            mock(self.ui, intent, Box::new(after));
            return;
        }
        dispatch_complete(
            self.ui,
            "card.move",
            serde_json::json!({
                "card_id": intent.card_id,
                "revision": intent.revision,
                "stage": intent.stage,
                "before_card_id": intent.before_card_id,
            }),
            after,
        );
    }
    fn clear(self) {
        if self.phase.try_get().is_none() {
            return;
        }
        let had_session = self.session.replace(None).is_some();
        self.phase.set(0);
        self.ghost.set(None);
        self.lifted.set(String::new());
        self.x.set(0.);
        self.y.set(0.);
        self.sync_with(false);
        if had_session {
            self.reflow();
        }
    }
    pub fn overlay(self) -> impl IntoElement {
        view()
            .absolute()
            .inset_0()
            .z_index(100)
            .pointer_events(PointerEvents::None)
            .child(each(
                move || self.flights.get(),
                |f| f.id.clone(),
                move |flight, _| {
                    let initial = flight.get_untracked();
                    let x = animated_value(0.);
                    let y = animated_value(0.);
                    let opacity = animated_value(initial.from_opacity);
                    let origin_x = initial.origin_x;
                    let origin_y = initial.origin_y;
                    let card_id = initial.id.clone();
                    let seen_gen = stored(0u64);
                    let prev_dx = stored(0f32);
                    let prev_dy = stored(0f32);
                    let prev_op = stored(initial.from_opacity);
                    effect(move || {
                        let spec = flight.get();
                        let prev_gen = seen_gen.replace(spec.generation);
                        if prev_gen == spec.generation {
                            return;
                        }
                        let first_run = prev_gen == 0;
                        let gen_id = spec.generation;
                        let dx = spec.target_x - origin_x;
                        let dy = spec.target_y - origin_y;
                        let dist = ((dx - prev_dx.replace(dx)).powi(2)
                            + (dy - prev_dy.replace(dy)).powi(2))
                        .sqrt();
                        let (end_x, end_y) = if spec.exiting {
                            (None, None)
                        } else {
                            let spring = transitions::flight_spring(dist);
                            if first_run {
                                let init_vel = spec.initial_velocity;
                                (
                                    motion::settle_from(
                                        x,
                                        init_vel.map(|(vx, _)| (0., vx)),
                                        dx,
                                        spring,
                                    ),
                                    motion::settle_from(
                                        y,
                                        init_vel.map(|(_, vy)| (0., vy)),
                                        dy,
                                        spring,
                                    ),
                                )
                            } else {
                                (motion::spring(x, dx, spring), motion::spring(y, dy, spring))
                            }
                        };
                        let op_ms = if spec.exiting {
                            transitions::EXIT_FADE_MS
                        } else {
                            transitions::FLIGHT_FADE_MS
                        };
                        let old_op = prev_op.replace(spec.to_opacity);
                        let end_op = if spec.exiting || (spec.to_opacity - old_op).abs() > 0.01 {
                            motion::timing(opacity, spec.to_opacity, op_ms, Easing::EaseOut)
                        } else {
                            None
                        };
                        let id = card_id.clone();
                        motion::when_done([end_x, end_y, end_op], move || {
                            self.finish_flight(&id, gen_id);
                        });
                    });
                    view()
                        .absolute()
                        .left(px(origin_x))
                        .top(px(origin_y))
                        .w(move || px(flight.get().width))
                        .pointer_events(PointerEvents::None)
                        .shadow_lg()
                        .rounded(px(RADIUS))
                        .bind_translate_x(&x)
                        .bind_translate_y(&y)
                        .bind_opacity(&opacity)
                        .child(dynamic(move || card_tile(self.ui, flight.get().card)))
                },
            ))
            .child(each(
                move || self.exits.get(),
                |e| e.id.clone(),
                move |exit, _| {
                    let initial = exit.get_untracked();
                    let opacity = animated_value(1.0);
                    let scale = animated_value(1.0);
                    let end_op =
                        motion::timing(opacity, 0.0, transitions::EXIT_FADE_MS, Easing::EaseOut);
                    let end_scale = motion::timing(
                        scale,
                        transitions::EXIT_SCALE_TO,
                        transitions::EXIT_FADE_MS,
                        Easing::EaseOut,
                    );
                    let id = initial.id.clone();
                    let gen_id = initial.generation;
                    motion::when_done([end_op, end_scale], move || {
                        self.finish_exit(&id, gen_id);
                    });
                    view()
                        .absolute()
                        .left(px(initial.x))
                        .top(px(initial.y))
                        .w(px(initial.width))
                        .pointer_events(PointerEvents::None)
                        .rounded(px(RADIUS))
                        .bind_opacity(&opacity)
                        .bind_scale(&scale)
                        .child(card_tile(self.ui, initial.card))
                },
            ))
            .child(dynamic(move || match self.ghost.get() {
                None => view().into_element(),
                Some((card, x, y, width)) => view()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(width))
                    .pointer_events(PointerEvents::None)
                    .shadow_lg()
                    .rounded(px(RADIUS))
                    .bind_translate_x(&self.x)
                    .bind_translate_y(&self.y)
                    .child(card_tile(self.ui, card))
                    .into_element(),
            }))
    }
}

#[cfg(test)]
type MoveRpcCallback = Box<dyn FnOnce(Result<serde_json::Value, String>)>;
#[cfg(test)]
type MoveRpcMock = std::rc::Rc<dyn Fn(Ui, Drop, MoveRpcCallback)>;
#[cfg(test)]
thread_local! {
    static TEST_MOVE_RPC: std::cell::RefCell<Option<MoveRpcMock>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Snapshot, Workflow};
    use canban_ui_kit::motion::ReducedMotion;
    use nsh_core::{
        ApproxTextEngine, Core, MountBatch, MountItem, NodeId,
        animation::{AnimValues, AnimationId, AnimationOp, Animator},
        element::JsEvent,
        geometry::size,
    };
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    #[component]
    fn DragBoardProbe() -> impl IntoElement {
        canban_ui_kit::tokens::install_theme();
        let ui = Ui::new();
        let mut snapshot = Snapshot::default();
        snapshot.projects.push(
            serde_json::from_value(serde_json::json!({
                "id": "proj-drag",
                "name": "Drag Probe",
                "directory": "/tmp/drag-probe",
                "workflow": Workflow::default(),
                "created_at": 1
            }))
            .unwrap(),
        );
        for n in 1u64..=3 {
            snapshot.cards.push(
                serde_json::from_value(serde_json::json!({
                    "id": format!("c-{n}"),
                    "project_id": "proj-drag",
                    "number": n,
                    "title": format!("Drag Card {n}"),
                    "body": "",
                    "stage": "backlog",
                    "revision": 1,
                    "created_at": n
                }))
                .unwrap(),
            );
        }
        ui.snapshot.set(snapshot);
        ui.project.set("proj-drag".into());
        ui.page.set("board".into());
        ui.onboarded.set(true);
        let pending_rpc: Rc<RefCell<Option<(Drop, MoveRpcCallback)>>> = Rc::new(RefCell::new(None));
        let capture = pending_rpc.clone();
        TEST_MOVE_RPC.with(|m| {
            *m.borrow_mut() = Some(Rc::new(move |ui_arg, intent, cb| {
                ui_arg.pending_methods.update(|s| {
                    s.insert("card.move");
                });
                *capture.borrow_mut() = Some((intent, cb));
            }));
        });
        let ack_rpc = pending_rpc.clone();
        let err_rpc = pending_rpc;
        view()
            .w(px(1200.))
            .h(px(760.))
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Ack Pending Move".into()))
                    .on_press(move |_: &()| {
                        let (intent, cb) =
                            ack_rpc.borrow_mut().take().expect("captured card.move RPC");
                        ui.pending_methods.update(|s| {
                            s.remove("card.move");
                        });
                        ui.snapshot.update(|s| {
                            super::model::apply_pending_move(
                                &mut s.cards,
                                &super::model::PendingMove {
                                    card_id: intent.card_id.clone(),
                                    base_revision: intent.revision,
                                    target_stage: intent.stage.clone(),
                                    before_card_id: intent.before_card_id.clone(),
                                },
                            );
                            if let Some(c) = s.cards.iter_mut().find(|c| c.id == intent.card_id) {
                                c.revision = intent.revision + 1;
                            }
                        });
                        cb(Ok(serde_json::json!({"accepted": true})));
                    }),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Reject Pending Move".into()))
                    .on_press(move |_: &()| {
                        let (intent, cb) =
                            err_rpc.borrow_mut().take().expect("captured card.move RPC");
                        assert_eq!(intent.stage, "develop");
                        ui.pending_methods.update(|s| {
                            s.remove("card.move");
                        });
                        cb(Err("Human review must be approved".into()));
                    }),
            )
            .child(crate::board::screen(ui))
    }

    struct DragTree {
        parents: HashMap<NodeId, NodeId>,
        labels: HashMap<String, NodeId>,
        texts: HashMap<NodeId, String>,
        frames: HashMap<NodeId, Bounds<Points>>,
        layout_listeners: std::collections::HashSet<NodeId>,
        delivered_frames: HashMap<NodeId, Bounds<Points>>,
        pending_layout_events: Vec<(NodeId, Bounds<Points>)>,
        opacities: HashMap<NodeId, f32>,
        keymap: Option<std::sync::Arc<nsh_core::keymap::Keymap>>,
        started_animations: Vec<AnimationId>,
        animator: Animator,
        anim_values: HashMap<NodeId, AnimValues>,
        clock: f64,
    }

    impl DragTree {
        fn new() -> Self {
            Self {
                parents: HashMap::new(),
                labels: HashMap::new(),
                texts: HashMap::new(),
                frames: HashMap::new(),
                layout_listeners: std::collections::HashSet::new(),
                delivered_frames: HashMap::new(),
                pending_layout_events: Vec::new(),
                opacities: HashMap::new(),
                keymap: None,
                started_animations: Vec::new(),
                animator: Animator::default(),
                anim_values: HashMap::new(),
                clock: 0.0,
            }
        }

        fn apply(&mut self, batch: &MountBatch) {
            for item in &batch.items {
                self.animator.apply(item);
                match item {
                    MountItem::Insert { parent, child, .. } => {
                        self.parents.insert(*child, *parent);
                    }
                    MountItem::Remove { id } => {
                        self.parents.remove(id);
                    }
                    MountItem::Delete { id } => {
                        self.parents.remove(id);
                        self.texts.remove(id);
                        self.frames.remove(id);
                        self.layout_listeners.remove(id);
                        self.delivered_frames.remove(id);
                        self.opacities.remove(id);
                        self.anim_values.remove(id);
                        self.labels.retain(|_, node| node != id);
                    }
                    MountItem::Text { id, text, .. } => {
                        self.texts.insert(*id, text.text.to_string());
                    }
                    MountItem::Frame { id, frame } => {
                        self.frames.insert(*id, *frame);
                        if self.layout_listeners.contains(id)
                            && self.delivered_frames.get(id) != Some(frame)
                        {
                            self.pending_layout_events.push((*id, *frame));
                        }
                    }
                    MountItem::Events { id, events } => {
                        if events.contains(EventKind::Layout) {
                            self.layout_listeners.insert(*id);
                            if let Some(&frame) = self.frames.get(id)
                                && self.delivered_frames.get(id) != Some(&frame)
                            {
                                self.pending_layout_events.push((*id, frame));
                            }
                        } else {
                            self.layout_listeners.remove(id);
                        }
                    }
                    MountItem::ViewProps { id, props } => {
                        if let Some(label) = &props.accessibility.label {
                            self.labels.insert(label.to_string(), *id);
                        }
                    }
                    MountItem::ViewStyle { id, style } => {
                        self.opacities.insert(*id, style.opacity);
                    }
                    MountItem::Keymap(keymap) => {
                        self.keymap = Some(keymap.clone());
                    }
                    MountItem::Animation(AnimationOp::Start { animation, .. }) => {
                        self.started_animations.push(*animation);
                    }
                    _ => {}
                }
            }
            self.sync_anim_values();
        }

        fn sync_anim_values(&mut self) {
            let map = &mut self.anim_values;
            self.animator.write(Some, |id, values, _| {
                map.insert(id, values);
            });
        }

        fn is_mounted(&self, mut id: NodeId) -> bool {
            let mut guard = 0;
            while id != nsh_core::ROOT_ID {
                let Some(&parent) = self.parents.get(&id) else {
                    return false;
                };
                id = parent;
                guard += 1;
                if guard > 256 {
                    return false;
                }
            }
            true
        }

        fn has_mounted_ghost(&self) -> bool {
            self.texts
                .iter()
                .filter(|(id, text)| text.as_str() == "Drag Card 1" && self.is_mounted(**id))
                .count()
                == 2
        }

        fn row_opacity(&self, row_id: NodeId) -> f32 {
            self.anim_values
                .get(&row_id)
                .and_then(|v| v.opacity)
                .unwrap_or_else(|| self.opacities.get(&row_id).copied().unwrap_or(1.0))
        }

        fn row_visual_y(&self, row_id: NodeId) -> f32 {
            let top = self
                .frames
                .get(&row_id)
                .map(|f| f.origin.y.0)
                .unwrap_or(0.0);
            let translate_y = self
                .anim_values
                .get(&row_id)
                .and_then(|v| v.transform[0])
                .unwrap_or(0.0);
            top + translate_y
        }
    }

    fn deliver_finished_animations(core: &mut Core<ApproxTextEngine>, tree: &mut DragTree) -> bool {
        if tree.animator.finished.is_empty() {
            return false;
        }
        core.animations_finished(&mut tree.animator.finished);
        true
    }

    fn dispatch_core_events(core: &mut Core<ApproxTextEngine>, tree: &mut DragTree) -> bool {
        let mut dispatched = false;
        let pending_layouts = std::mem::take(&mut tree.pending_layout_events);
        for (id, frame) in pending_layouts {
            if tree.delivered_frames.get(&id) == Some(&frame) {
                continue;
            }
            tree.delivered_frames.insert(id, frame);
            core.component_event(id, EventKind::Layout, &EventPayload::Layout(frame));
            dispatched = true;
        }
        let mut events = Vec::new();
        core.take_events(&mut events);
        for ev in events {
            match ev {
                JsEvent::Event(id, kind, payload) => {
                    if kind == EventKind::Layout
                        && let EventPayload::Layout(frame) = &payload
                    {
                        tree.delivered_frames.insert(id, *frame);
                    }
                    core.component_event(id, kind, &payload);
                    dispatched = true;
                }
                JsEvent::LayoutListener {
                    host,
                    id,
                    listener,
                    frame,
                } => {
                    tree.delivered_frames.insert(id, frame);
                    core.component_layout(host, id, &listener, frame);
                    dispatched = true;
                }
                _ => {}
            }
        }
        dispatched
    }

    fn pump_board(core: &mut Core<ApproxTextEngine>, tree: &mut DragTree) {
        for _ in 0..16 {
            core.run_frame(nsh_core::time::now());
            core.run_component_tasks();
            let had_batch = if let Some(batch) = core.commit() {
                tree.apply(&batch);
                true
            } else {
                false
            };
            let had_anim = deliver_finished_animations(core, tree);
            let dispatched = dispatch_core_events(core, tree);
            if !had_batch && !had_anim && !dispatched && !core.wants_frame() {
                break;
            }
        }
    }

    fn step_animator_ms(core: &mut Core<ApproxTextEngine>, tree: &mut DragTree, total_ms: u64) {
        let frames = (total_ms / 16).max(1);
        let dt = (total_ms as f64 / 1000.0) / (frames as f64);
        for _ in 0..frames {
            tree.clock += dt;
            tree.animator.tick(tree.clock);
            tree.sync_anim_values();
            deliver_finished_animations(core, tree);
            pump_board(core, tree);
        }
    }

    fn finish_all_animations(
        core: &mut Core<ApproxTextEngine>,
        tree: &mut DragTree,
        _runtime: &tokio::runtime::Runtime,
        done: impl Fn(&DragTree) -> bool,
    ) {
        for _ in 0..120 {
            if !tree.animator.finishing() && tree.animator.finished.is_empty() && done(tree) {
                break;
            }
            step_animator_ms(core, tree, 16);
        }
    }

    fn setup_board(
        runtime: &tokio::runtime::Runtime,
    ) -> (Core<ApproxTextEngine>, DragTree, NodeId, Point<Points>) {
        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(760.)));
        core.set_components(nsh_base::registry());
        core.set_task_runtime(runtime.handle().clone());
        core.mount_root(&nsh_core::element::Root::of::<DragBoardProbe>());
        let mut tree = DragTree::new();
        pump_board(&mut core, &mut tree);
        // Settle the initial post-mount height reflow (`DEFAULT_HEIGHT` -> measured row height)
        // before running drag assertions.
        core.run_frame(nsh_core::time::now() + std::time::Duration::from_millis(20));
        pump_board(&mut core, &mut tree);
        tree.started_animations.clear();
        let row_id = *tree
            .labels
            .get("Card #1 Drag Card 1")
            .expect("first card row mounted");
        let row_bounds = core.measure(row_id).expect("measured row bounds");
        let press_pos = point(px(row_bounds.page.x.0 + 40.), px(row_bounds.page.y.0 + 10.));
        (core, tree, row_id, press_pos)
    }

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

    #[test]
    fn settle_short_circuits_axis_already_at_rest() {
        assert!(motion::should_short_circuit_settle(Some((0.0, 0.0)), 0.0));
        assert!(motion::should_short_circuit_settle(Some((0.4, -5.0)), 0.0));
        assert!(!motion::should_short_circuit_settle(Some((12.0, 0.0)), 0.0));
        assert!(!motion::should_short_circuit_settle(Some((0.0, 25.0)), 0.0));
        assert!(!motion::should_short_circuit_settle(None, 0.0));
    }

    #[test]
    fn short_drag_releasing_during_phase_1_never_hides_card_or_flashes_ghost() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (mut core, mut tree, row_id, press_pos) = setup_board(&runtime);

        assert!(core.component_event(
            row_id,
            EventKind::PressDown,
            &EventPayload::Press(PressEvent {
                position: point(px(40.), px(10.)),
                page_position: press_pos,
                ..Default::default()
            }),
        ));
        assert!(core.component_event(
            row_id,
            EventKind::PanStart,
            &EventPayload::Pan {
                translation: point(px(0.), px(7.)),
                velocity_x: 0.,
                velocity_y: 40.,
                value_x: Some(px(0.)),
                value_y: Some(px(7.)),
            },
        ));
        // Release slightly below start while measurements are still in phase 1.
        assert!(core.component_event(
            row_id,
            EventKind::PanEnd,
            &EventPayload::Pan {
                translation: point(px(0.), px(12.)),
                velocity_x: 0.,
                velocity_y: 0.,
                value_x: Some(px(0.)),
                value_y: Some(px(12.)),
            },
        ));

        for step_idx in 0..8 {
            core.run_component_tasks();
            if let Some(batch) = core.commit() {
                tree.apply(&batch);
            }
            dispatch_core_events(&mut core, &mut tree);
            assert_eq!(
                tree.row_opacity(row_id),
                1.0,
                "step {step_idx}: original row must never vanish when a short drag ends in phase 1; has_ghost={} started_anims={:?}",
                tree.has_mounted_ghost(),
                tree.started_animations,
            );
            assert!(
                !tree.has_mounted_ghost(),
                "step {step_idx}: short unchanged drag ended in phase 1 should clear without flashing ghost"
            );
        }
        assert!(
            tree.started_animations.is_empty(),
            "short unchanged drag ended in phase 1 must not wait on spring animations"
        );
    }

    #[test]
    fn drag_start_keeps_row_visible_until_single_commit_ghost_handoff_and_skips_zero_axis_spring() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (mut core, mut tree, row_id, press_pos) = setup_board(&runtime);

        assert!(core.component_event(
            row_id,
            EventKind::PressDown,
            &EventPayload::Press(PressEvent {
                position: point(px(40.), px(10.)),
                page_position: press_pos,
                ..Default::default()
            }),
        ));
        assert!(core.component_event(
            row_id,
            EventKind::PanStart,
            &EventPayload::Pan {
                translation: point(px(0.), px(7.)),
                velocity_x: 0.,
                velocity_y: 40.,
                value_x: Some(px(0.)),
                value_y: Some(px(7.)),
            },
        ));

        // Run tasks once so start() issues all concurrent measure() requests, then commit 1:
        // card stays visible at opacity 1.0 while measurements are in flight.
        core.run_component_tasks();
        if let Some(batch) = core.commit() {
            tree.apply(&batch);
        }
        assert_eq!(
            tree.row_opacity(row_id),
            1.0,
            "original row must stay visible during phase 1 before ghost mounts"
        );
        assert!(!tree.has_mounted_ghost());

        // Commit 1 answered all concurrent measurements (source, root, and all columns);
        // running tasks + Commit 2 atomically mounts ghost and hides the original row.
        core.run_component_tasks();
        if let Some(batch) = core.commit() {
            tree.apply(&batch);
        }
        assert!(
            tree.has_mounted_ghost(),
            "ghost must mount after a single parallel measurement commit"
        );
        assert_eq!(
            tree.row_opacity(row_id),
            0.0,
            "original row hides in the exact batch ghost mounts"
        );

        // Release slightly downward in phase 2: only Y should spring home (X is already 0.0).
        tree.started_animations.clear();
        assert!(core.component_event(
            row_id,
            EventKind::PanEnd,
            &EventPayload::Pan {
                translation: point(px(0.), px(14.)),
                velocity_x: 0.,
                velocity_y: 0.,
                value_x: Some(px(0.)),
                value_y: Some(px(14.)),
            },
        ));
        pump_board(&mut core, &mut tree);
        assert_eq!(
            tree.started_animations.len(),
            1,
            "unchanged vertical drag should spring only Y and short-circuit zero-delta X; started={:?}",
            tree.started_animations
        );
        assert!(tree.has_mounted_ghost());
        assert_eq!(tree.row_opacity(row_id), 0.0);

        finish_all_animations(&mut core, &mut tree, &runtime, |t| !t.has_mounted_ghost());

        assert!(
            !tree.has_mounted_ghost(),
            "ghost must unmount once return-home motion completes"
        );
        assert_eq!(
            tree.row_opacity(row_id),
            1.0,
            "original row must return to opacity 1.0 in the same clear batch"
        );
    }

    #[component]
    fn TransitionBoardProbe() -> impl IntoElement {
        canban_ui_kit::tokens::install_theme();
        let reduced = signal(false);
        provide_context(ReducedMotion(reduced));
        let ui = Ui::new();
        let mut snapshot = Snapshot::default();
        snapshot.projects.push(
            serde_json::from_value(serde_json::json!({
                "id": "proj-trans",
                "name": "Transition Probe",
                "directory": "/tmp/trans-probe",
                "workflow": Workflow::default(),
                "created_at": 1
            }))
            .unwrap(),
        );
        for n in 1u64..=3 {
            snapshot.cards.push(
                serde_json::from_value(serde_json::json!({
                    "id": format!("c-{n}"),
                    "project_id": "proj-trans",
                    "number": n,
                    "title": format!("Drag Card {n}"),
                    "body": "",
                    "stage": "backlog",
                    "revision": 1,
                    "created_at": n
                }))
                .unwrap(),
            );
        }
        ui.snapshot.set(snapshot);
        ui.project.set("proj-trans".into());
        ui.page.set("board".into());
        ui.onboarded.set(true);
        view()
            .w(px(1200.))
            .h(px(760.))
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Move c-1 to develop".into()))
                    .on_press(move |_: &()| {
                        ui.snapshot.update(|s| {
                            if let Some(c) = s.cards.iter_mut().find(|c| c.id == "c-1") {
                                c.stage = "develop".into();
                                c.revision += 1;
                            }
                        });
                    }),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Move c-1 to review".into()))
                    .on_press(move |_: &()| {
                        ui.snapshot.update(|s| {
                            if let Some(c) = s.cards.iter_mut().find(|c| c.id == "c-1") {
                                c.stage = "review".into();
                                c.revision += 1;
                            }
                        });
                    }),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Create Born Card".into()))
                    .on_press(move |_: &()| {
                        ui.snapshot.update(|s| {
                            s.cards.push(
                                serde_json::from_value(serde_json::json!({
                                    "id": "c-born",
                                    "project_id": "proj-trans",
                                    "number": 99,
                                    "title": "Born Card",
                                    "body": "",
                                    "stage": "develop",
                                    "revision": 1,
                                    "created_at": 99
                                }))
                                .unwrap(),
                            );
                        });
                    }),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Archive Born Card".into()))
                    .on_press(move |_: &()| {
                        ui.snapshot.update(|s| {
                            if let Some(c) = s.cards.iter_mut().find(|c| c.id == "c-born") {
                                c.archived_at = Some(1000);
                                c.revision += 1;
                            }
                        });
                    }),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Toggle Reduced Motion".into()))
                    .on_press(move |_: &()| reduced.set(!reduced.get_untracked())),
            )
            .child(
                canban_ui_kit::press_target()
                    .accessibility_label(Some("Nudge Review Top".into()))
                    .on_press(move |_: &()| {
                        ui.snapshot.update(|s| {
                            s.cards.push(
                                serde_json::from_value(serde_json::json!({
                                    "id": "c-review-top",
                                    "project_id": "proj-trans",
                                    "number": 50,
                                    "title": "Review Top Card",
                                    "body": "",
                                    "stage": "review",
                                    "board_order": -1024,
                                    "revision": 1,
                                    "created_at": 50
                                }))
                                .unwrap(),
                            );
                        });
                    }),
            )
            .child(crate::board::screen(ui))
    }

    fn setup_transition_board(
        runtime: &tokio::runtime::Runtime,
    ) -> (Core<ApproxTextEngine>, DragTree) {
        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(760.)));
        core.set_components(nsh_base::registry());
        core.set_task_runtime(runtime.handle().clone());
        core.mount_root(&nsh_core::element::Root::of::<TransitionBoardProbe>());
        let mut tree = DragTree::new();
        pump_board(&mut core, &mut tree);
        core.run_frame(nsh_core::time::now() + std::time::Duration::from_millis(20));
        pump_board(&mut core, &mut tree);
        tree.started_animations.clear();
        (core, tree)
    }

    fn press_labeled(core: &mut Core<ApproxTextEngine>, tree: &DragTree, label: &str) {
        let id = *tree.labels.get(label).expect("probe button");
        assert!(core.component_event(id, EventKind::Press, &EventPayload::None));
    }

    fn mounted_text_count(tree: &DragTree, text: &str) -> usize {
        tree.texts
            .iter()
            .filter(|(id, t)| t.as_str() == text && tree.is_mounted(**id))
            .count()
    }

    #[test]
    fn stage_update_launches_flight_ghost_hides_target_and_restores_on_finish() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (mut core, mut tree) = setup_transition_board(&runtime);
        tree.started_animations.clear();

        press_labeled(&mut core, &tree, "Move c-1 to develop");
        pump_board(&mut core, &mut tree);

        assert!(
            tree.has_mounted_ghost(),
            "cross-column stage update must mount flight ghost alongside destination row"
        );
        let new_row_id = *tree
            .labels
            .get("Card #1 Drag Card 1")
            .expect("destination row mounted");
        assert_eq!(
            tree.row_opacity(new_row_id),
            0.0,
            "destination row stays hidden while flight ghost travels"
        );
        assert!(
            !tree.started_animations.is_empty(),
            "cross-column move should start flight and peer FLIP animations"
        );

        finish_all_animations(&mut core, &mut tree, &runtime, |t| !t.has_mounted_ghost());
        assert!(
            !tree.has_mounted_ghost(),
            "flight ghost must unmount after animation finishes"
        );
        assert_eq!(
            tree.row_opacity(new_row_id),
            1.0,
            "destination row must return to opacity 1.0 when flight completes"
        );
    }

    #[test]
    fn rapid_stage_updates_retarget_flight_without_leaving_hidden_row() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (mut core, mut tree) = setup_transition_board(&runtime);
        tree.started_animations.clear();

        press_labeled(&mut core, &tree, "Move c-1 to develop");
        pump_board(&mut core, &mut tree);
        assert!(tree.has_mounted_ghost());

        press_labeled(&mut core, &tree, "Move c-1 to review");
        pump_board(&mut core, &mut tree);
        assert!(tree.has_mounted_ghost());
        let review_row_id = *tree
            .labels
            .get("Card #1 Drag Card 1")
            .expect("review row mounted");
        assert_eq!(tree.row_opacity(review_row_id), 0.0);

        finish_all_animations(&mut core, &mut tree, &runtime, |t| !t.has_mounted_ghost());
        assert!(!tree.has_mounted_ghost());
        assert_eq!(tree.row_opacity(review_row_id), 1.0);
    }

    #[test]
    fn creating_and_archiving_card_plays_entry_and_fade_out_animations() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (mut core, mut tree) = setup_transition_board(&runtime);
        tree.started_animations.clear();

        press_labeled(&mut core, &tree, "Create Born Card");
        pump_board(&mut core, &mut tree);
        assert_eq!(mounted_text_count(&tree, "Born Card"), 1);
        assert!(
            tree.started_animations.len() >= 2,
            "new card creation should start pop-in scale and opacity animations; started={:?}",
            tree.started_animations
        );

        finish_all_animations(&mut core, &mut tree, &runtime, |_| true);
        tree.started_animations.clear();

        press_labeled(&mut core, &tree, "Archive Born Card");
        pump_board(&mut core, &mut tree);
        assert_eq!(
            mounted_text_count(&tree, "Born Card"),
            1,
            "archived card should remain mounted in exit overlay while fading out"
        );
        assert!(
            tree.started_animations.len() >= 2,
            "archiving should start fade-out opacity and scale animations"
        );

        finish_all_animations(&mut core, &mut tree, &runtime, |t| {
            mounted_text_count(t, "Born Card") == 0
        });
        assert_eq!(
            mounted_text_count(&tree, "Born Card"),
            0,
            "exit ghost must unmount once fade-out completes"
        );
    }

    #[test]
    fn reduced_motion_skips_board_transition_ghosts_and_animations() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (mut core, mut tree) = setup_transition_board(&runtime);

        press_labeled(&mut core, &tree, "Toggle Reduced Motion");
        pump_board(&mut core, &mut tree);
        tree.started_animations.clear();

        press_labeled(&mut core, &tree, "Move c-1 to develop");
        pump_board(&mut core, &mut tree);

        assert!(
            !tree.has_mounted_ghost(),
            "reduced motion must not mount flight ghosts"
        );
        assert!(
            tree.started_animations.is_empty(),
            "reduced motion must not start transition animations"
        );
        let row_id = *tree
            .labels
            .get("Card #1 Drag Card 1")
            .expect("destination row mounted");
        assert_eq!(tree.row_opacity(row_id), 1.0);
    }

    #[test]
    fn cross_column_drop_pending_move_and_async_snapshot_never_vanish_card_or_double_animate() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();

        let (mut core, mut tree, row_id, press_pos) = setup_board(&runtime);
        let peer_row_id = *tree
            .labels
            .get("Card #2 Drag Card 2")
            .expect("second card row mounted");
        let initial_peer_y = tree.row_visual_y(peer_row_id);
        assert!(initial_peer_y > 40.0);

        // Start dragging Card #1 from Backlog -> Develop (~392pt wide + 12pt gap -> dx = 420pt).
        assert!(core.component_event(
            row_id,
            EventKind::PressDown,
            &EventPayload::Press(PressEvent {
                position: point(px(40.), px(10.)),
                page_position: press_pos,
                ..Default::default()
            }),
        ));
        assert!(core.component_event(
            row_id,
            EventKind::PanStart,
            &EventPayload::Pan {
                translation: point(px(10.), px(0.)),
                velocity_x: 120.,
                velocity_y: 0.,
                value_x: Some(px(10.)),
                value_y: Some(px(0.)),
            },
        ));
        pump_board(&mut core, &mut tree);

        assert!(core.component_event(
            row_id,
            EventKind::PanEnd,
            &EventPayload::Pan {
                translation: point(px(420.), px(24.)),
                velocity_x: 80.,
                velocity_y: -20.,
                value_x: Some(px(420.)),
                value_y: Some(px(24.)),
            },
        ));
        pump_board(&mut core, &mut tree);

        // Optimistic PendingMove took effect immediately on drop:
        // 1. Flight ghost is mounted while destination row in Develop has opacity 0.0.
        assert!(
            tree.has_mounted_ghost(),
            "released card must hand off seamlessly to flight overlay"
        );
        let dest_row_id = *tree
            .labels
            .get("Card #1 Drag Card 1")
            .expect("destination row mounted in develop");
        assert_eq!(tree.row_opacity(dest_row_id), 0.0);

        // Step animations to completion BEFORE the server RPC responds (simulating slow SSH RTT
        // or stale snapshot prior to refresh).
        finish_all_animations(&mut core, &mut tree, &runtime, |t| !t.has_mounted_ghost());
        assert!(
            !tree.has_mounted_ghost(),
            "settle flight finishes cleanly without waiting on card.move RPC"
        );
        let settled_row_id = *tree
            .labels
            .get("Card #1 Drag Card 1")
            .expect("settled card row mounted");
        assert!(tree.is_mounted(settled_row_id));
        assert_eq!(
            tree.row_opacity(settled_row_id),
            1.0,
            "settled card must stay visible at opacity 1.0 while RPC is still in flight"
        );
        assert!(
            tree.row_visual_y(peer_row_id).abs() < 1.0,
            "source peer c-2 must stay shifted up at y=0 without snapping back to a hole; got {}",
            tree.row_visual_y(peer_row_id)
        );

        // Now complete card.move RPC + authoritative Snapshot refresh: ZERO duplicate flights!
        tree.started_animations.clear();
        press_labeled(&mut core, &tree, "Ack Pending Move");
        pump_board(&mut core, &mut tree);

        assert!(
            !tree.has_mounted_ghost(),
            "authoritative snapshot arrival must NOT start a second cross-column flight"
        );
        assert!(
            tree.started_animations.is_empty(),
            "authoritative snapshot arrival matching PendingMove must start zero animations; got {:?}",
            tree.started_animations
        );
        assert_eq!(tree.row_opacity(settled_row_id), 1.0);
        assert!(tree.row_visual_y(peer_row_id).abs() < 1.0);

        TEST_MOVE_RPC.with(|m| *m.borrow_mut() = None);
    }

    #[test]
    fn failed_card_move_clears_pending_move_and_flies_card_back_to_source_column() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();

        let (mut core, mut tree, row_id, press_pos) = setup_board(&runtime);
        let peer_row_id = *tree
            .labels
            .get("Card #2 Drag Card 2")
            .expect("second card row mounted");
        let settled_peer_y = tree.row_visual_y(peer_row_id);
        assert!(settled_peer_y > 40.0);

        assert!(core.component_event(
            row_id,
            EventKind::PressDown,
            &EventPayload::Press(PressEvent {
                position: point(px(40.), px(10.)),
                page_position: press_pos,
                ..Default::default()
            }),
        ));
        assert!(core.component_event(
            row_id,
            EventKind::PanStart,
            &EventPayload::Pan {
                translation: point(px(10.), px(0.)),
                velocity_x: 0.,
                velocity_y: 0.,
                value_x: Some(px(10.)),
                value_y: Some(px(0.)),
            },
        ));
        pump_board(&mut core, &mut tree);

        assert!(core.component_event(
            row_id,
            EventKind::PanEnd,
            &EventPayload::Pan {
                translation: point(px(420.), px(10.)),
                velocity_x: 0.,
                velocity_y: 0.,
                value_x: Some(px(420.)),
                value_y: Some(px(10.)),
            },
        ));
        pump_board(&mut core, &mut tree);
        finish_all_animations(&mut core, &mut tree, &runtime, |t| !t.has_mounted_ghost());
        assert!(tree.row_visual_y(peer_row_id).abs() < 1.0);

        // Reject the RPC: PendingMove clears and launches a rollback FlightSpec back to Backlog!
        tree.started_animations.clear();
        press_labeled(&mut core, &tree, "Reject Pending Move");
        pump_board(&mut core, &mut tree);

        assert!(
            tree.has_mounted_ghost(),
            "rejected card.move must launch a smooth rollback flight back to original column"
        );
        assert!(
            !tree.started_animations.is_empty(),
            "rollback must start flight and peer displacement springs"
        );

        finish_all_animations(&mut core, &mut tree, &runtime, |t| !t.has_mounted_ghost());
        assert!(!tree.has_mounted_ghost());
        let restored_row_id = *tree
            .labels
            .get("Card #1 Drag Card 1")
            .expect("restored backlog row");
        assert_eq!(tree.row_opacity(restored_row_id), 1.0);
        assert!(
            (tree.row_visual_y(peer_row_id) - settled_peer_y).abs() < 1.0,
            "peer c-2 must return to {settled_peer_y} after rollback; got {}",
            tree.row_visual_y(peer_row_id)
        );

        TEST_MOVE_RPC.with(|m| *m.borrow_mut() = None);
    }

    #[test]
    fn mid_flight_vertical_retarget_keeps_flight_mounted_until_horizontal_axis_finishes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (mut core, mut tree) = setup_transition_board(&runtime);

        // Move c-1 far horizontally to Review (large dx ~780pt -> softer stiffness ~912).
        press_labeled(&mut core, &tree, "Move c-1 to review");
        pump_board(&mut core, &mut tree);
        assert!(tree.has_mounted_ghost());

        // Step 16ms so X is early in its long flight, then insert a card above c-1 in Review:
        // ONLY target_y changes (+124pt, stiffer spring ~1322) while target_x is unchanged!
        step_animator_ms(&mut core, &mut tree, 16);
        assert!(tree.has_mounted_ghost());

        press_labeled(&mut core, &tree, "Nudge Review Top");
        pump_board(&mut core, &mut tree);
        assert!(tree.has_mounted_ghost());

        let review_row_id = *tree
            .labels
            .get("Card #1 Drag Card 1")
            .expect("review row mounted");

        // Step 176ms: the short 124pt Y retarget spring finishes first while the ~780pt X
        // flight is still several points away from rest. Before unconditional dual-axis
        // retargeting, Y finishing alone unmounted the flight ghost mid-air!
        step_animator_ms(&mut core, &mut tree, 176);
        assert!(
            tree.has_mounted_ghost(),
            "flight ghost must remain mounted while X is still mid-flight after Y finishes"
        );
        assert_eq!(
            tree.row_opacity(review_row_id),
            0.0,
            "destination row must stay hidden until BOTH X and Y springs finish"
        );

        finish_all_animations(&mut core, &mut tree, &runtime, |t| !t.has_mounted_ghost());
        assert!(!tree.has_mounted_ghost());
        assert_eq!(tree.row_opacity(review_row_id), 1.0);
    }

    #[test]
    fn same_column_at_rest_drop_unmasks_without_bogus_spring_and_escape_flies_home() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();

        let (mut core, mut tree, row_id, press_pos) = setup_board(&runtime);
        let row3_id = *tree
            .labels
            .get("Card #3 Drag Card 3")
            .expect("third card row mounted");
        let slot2_y = tree.row_visual_y(row3_id);
        assert!(slot2_y > 80.0);

        // 1. Drag Card #1 to exact resting slot 2 (`dy == slot2_y`, zero velocity) and drop:
        // `make_release_flight` short-circuits, and Card #1 must unmask directly at `slot2_y`
        // without springing from its old slot 0!
        assert!(core.component_event(
            row_id,
            EventKind::PressDown,
            &EventPayload::Press(PressEvent {
                position: point(px(40.), px(10.)),
                page_position: press_pos,
                ..Default::default()
            }),
        ));
        pump_board(&mut core, &mut tree);
        tree.animator.pan_began(row_id);
        tree.animator.pan_moved(row_id, point(px(0.), px(slot2_y)));
        assert!(core.component_event(
            row_id,
            EventKind::PanStart,
            &EventPayload::Pan {
                translation: point(px(0.), px(slot2_y)),
                velocity_x: 0.,
                velocity_y: 0.,
                value_x: Some(px(0.)),
                value_y: Some(px(slot2_y)),
            },
        ));
        pump_board(&mut core, &mut tree);
        finish_all_animations(&mut core, &mut tree, &runtime, |_| true);
        tree.started_animations.clear();

        assert!(core.component_event(
            row_id,
            EventKind::PanEnd,
            &EventPayload::Pan {
                translation: point(px(0.), px(slot2_y)),
                velocity_x: 0.,
                velocity_y: 0.,
                value_x: Some(px(0.)),
                value_y: Some(px(slot2_y)),
            },
        ));
        pump_board(&mut core, &mut tree);

        assert!(
            !tree.has_mounted_ghost(),
            "at-rest drop right on target slot must short-circuit flight overlay"
        );
        assert_eq!(tree.row_opacity(row_id), 1.0);
        assert!(
            (tree.row_visual_y(row_id) - slot2_y).abs() < 1.0,
            "unmasked card must sit directly at slot2_y={slot2_y}, got {}",
            tree.row_visual_y(row_id)
        );
        press_labeled(&mut core, &tree, "Ack Pending Move");
        pump_board(&mut core, &mut tree);
        finish_all_animations(&mut core, &mut tree, &runtime, |_| true);

        // 2. Drag Card #1 far right and cancel via Escape (`cancelled: true` with default PanEvent):
        // must launch a return-home flight from the tracked pan coordinates!
        let moved_bounds = core.measure(row_id).expect("moved row bounds");
        let press_pos2 = point(
            px(moved_bounds.page.x.0 + 40.),
            px(moved_bounds.page.y.0 + 10.),
        );
        assert!(core.component_event(
            row_id,
            EventKind::PressDown,
            &EventPayload::Press(PressEvent {
                position: point(px(40.), px(10.)),
                page_position: press_pos2,
                ..Default::default()
            }),
        ));
        pump_board(&mut core, &mut tree);
        tree.animator.pan_began(row_id);
        tree.animator.pan_moved(row_id, point(px(260.), px(-40.)));
        assert!(core.component_event(
            row_id,
            EventKind::PanStart,
            &EventPayload::Pan {
                translation: point(px(260.), px(-40.)),
                velocity_x: 0.,
                velocity_y: 0.,
                value_x: Some(px(260.)),
                value_y: Some(px(-40.)),
            },
        ));
        pump_board(&mut core, &mut tree);
        assert!(tree.has_mounted_ghost());

        let escape_id = tree
            .keymap
            .as_ref()
            .and_then(|km| {
                km.bindings()
                    .iter()
                    .rfind(|b| b.keystroke.key == nsh_core::keymap::Key::Escape)
                    .map(|b| b.id)
            })
            .expect("escape keybinding mounted during phase 2");
        tree.started_animations.clear();
        assert!(core.component_event(
            nsh_core::ROOT_ID,
            EventKind::KeyCommand,
            &EventPayload::Value(nsh_core::PayloadValue::Number(escape_id as f64)),
        ));
        pump_board(&mut core, &mut tree);
        assert!(
            tree.has_mounted_ghost(),
            "Escape cancel from displaced drag must launch return-home flight"
        );
        assert!(!tree.started_animations.is_empty());

        finish_all_animations(&mut core, &mut tree, &runtime, |t| !t.has_mounted_ghost());
        assert!(!tree.has_mounted_ghost());
        assert_eq!(tree.row_opacity(row_id), 1.0);
        assert!((tree.row_visual_y(row_id) - slot2_y).abs() < 1.0);

        TEST_MOVE_RPC.with(|m| *m.borrow_mut() = None);
    }
}
