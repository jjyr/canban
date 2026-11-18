//! Stable column and Card owners; only animated offsets change during a drag.
use super::{
    drag::BoardDrag,
    transitions::{self, EntryKind},
};
use crate::{
    components::card_tile::{CardTileModel, card_tile_from_model},
    drafts::*,
    presenter::*,
};
use canban_types::{Card, Stage};
use canban_ui_kit::{motion, tokens::*, *};
use nsh_core::prelude::*;
/// Narrowest column before the Board scrolls or switches to one stage.
pub(super) const COLUMN_MIN: f32 = 220.;
/// Widest column; wider windows keep Card proportions instead of stretching.
const COLUMN_MAX: f32 = 420.;
pub(super) fn column(ui: Ui, state: BoardDrag, stage: Stage, compact: bool) -> impl IntoElement {
    let id = stage.id.clone();
    let row_stage = id.clone();
    let height_stage = id.clone();
    let empty_stage = id.clone();
    let viewport = element_ref();
    let scroll_ref = element_ref();
    state.register(&id, viewport, scroll_ref);
    let key = format!("board:{}:{id}", ui.project.get_untracked());
    let position = scroll_position(ui, &key);
    let restored = stored(false);
    let gap_y = animated_value(0.);
    let gap_stage = id.clone();
    let prev_gap = stored(None::<f32>);
    effect(move || {
        if let Some((s, y, _)) = &state.layout.get().gap
            && s == &gap_stage
        {
            if prev_gap
                .replace(Some(*y))
                .is_some_and(|old| (old - *y).abs() >= 0.5)
            {
                let _ = motion::spring(gap_y, *y, motion::settle_spring());
            } else {
                gap_y.set(*y);
            }
        } else {
            prev_gap.replace(None);
        }
    });
    let gap_visible_stage = id.clone();
    let gap_height_stage = id.clone();
    let list_stage = id.clone();
    let viewport_stage = id.clone();
    let count_stage = id.clone();
    let is_backlog = stage.id == "backlog";
    let mut root = view()
        .w(px(260.))
        .flex_1()
        .min_w(px(COLUMN_MIN))
        .min_h(px(0.))
        .gap_3();
    if !compact {
        root = root.max_w(px(COLUMN_MAX));
    }
    // Compact mode shows the stage tabs above the column instead.
    let pill_stage = id.clone();
    root.children((!compact).then(move || {
        view()
            .flex_row()
            .min_h(px(30.))
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .min_w(px(0.))
                    .child(
                        label(stage.name)
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate(),
                    )
                    .child(
                        view()
                            .flex_shrink_0()
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(10.))
                            .bg(INSET)
                            .child(muted(move || {
                                let visible = state.cards.with(|all| {
                                    all.iter().filter(|c| c.stage == count_stage).count()
                                });
                                let pid = ui.project.get();
                                let total = ui.snapshot.with(|s| {
                                    s.cards
                                        .iter()
                                        .filter(|c| {
                                            c.project_id == pid
                                                && c.active()
                                                && c.stage == count_stage
                                        })
                                        .count()
                                });
                                if visible == total {
                                    visible.to_string()
                                } else {
                                    format!("{visible} / {total}")
                                }
                            })),
                    ),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .min_w(px(0.))
                    .child(crate::workflows::column_automation_pill(ui, pill_stage))
                    .child(when(
                        move || is_backlog,
                        move || chrome_button(Icon::Plus, "Add Card", move |_| ui.new_card()),
                    )),
            )
    }))
    .child(
        view()
            .element_ref(&viewport)
            .flex_1()
            .min_h(px(0.))
            .relative()
            .on_layout(move |b: &Bounds<Points>| state.viewport(&viewport_stage, b.size.height.0))
            .child(
                scroll()
                    .element_ref(&scroll_ref)
                    .scrollbar(false)
                    .flex_1()
                    .on_scroll(move |e: &ScrollEvent| {
                        state.scrolled(&id, e.offset.y.0);
                        capture_scroll(ui, key.clone(), e);
                    })
                    .child(
                        view()
                            .relative()
                            .w_full()
                            .on_layout(move |_: &Bounds<Points>| {
                                if !restored.replace(true) && position.y.0 > 0. {
                                    spawn(async move {
                                        next_frame().await;
                                        scroll_ref.scroll_to(position, false);
                                    });
                                }
                            })
                            .h(move || {
                                px(state.layout.with(|l| {
                                    l.heights.get(&height_stage).copied().unwrap_or(0.) + 2.
                                }))
                            })
                            .child(
                                view()
                                    .absolute()
                                    .left(px(0.))
                                    .right(px(0.))
                                    .top(px(0.))
                                    .pointer_events(PointerEvents::None)
                                    .bind_translate_y(&gap_y)
                                    .rounded(px(RADIUS))
                                    .bg(INSET)
                                    .opacity(move || {
                                        if state.layout.with(|l| {
                                            l.gap
                                                .as_ref()
                                                .is_some_and(|(s, _, _)| s == &gap_visible_stage)
                                        }) {
                                            0.8_f32
                                        } else {
                                            0.0_f32
                                        }
                                    })
                                    .h(move || {
                                        px(state.layout.with(|l| {
                                            l.gap
                                                .as_ref()
                                                .filter(|(s, _, _)| s == &gap_height_stage)
                                                .map(|(_, _, h)| *h)
                                                .unwrap_or(0.)
                                        }))
                                    }),
                            )
                            .child(each(
                                move || state.visible(&list_stage),
                                |c| c.id.clone(),
                                move |card, _| row(ui, state, card),
                            )),
                    ),
            )
            .child(when(
                move || {
                    state
                        .cards
                        .with(|all| !all.iter().any(|c| c.stage == empty_stage))
                        && state
                            .layout
                            .with(|l| l.gap.as_ref().is_none_or(|(s, _, _)| s != &row_stage))
                },
                move || {
                    view()
                        .pointer_events(PointerEvents::None)
                        .absolute()
                        .top(px(0.))
                        .left(px(0.))
                        .right(px(0.))
                        .py_5()
                        .px_3()
                        .rounded(px(RADIUS))
                        .border_1()
                        .border_color(BORDER)
                        .bg(INSET)
                        .opacity(0.65)
                        .items_center()
                        .justify_center()
                        .child(muted("No cards"))
                },
            ))
            .child(
                vertical_scrollbar(scroll_ref)
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(-8.)),
            ),
    )
}
fn row(ui: Ui, state: BoardDrag, card: ReadSignal<Card>) -> impl IntoElement {
    let reference = element_ref();
    let y = animated_value(0.);
    let id = card.get_untracked().id;
    let position_id = id.clone();
    let top_id = id.clone();
    let hidden_id = id.clone();
    let press_id = id.clone();
    let layout_id = id.clone();
    let pan_id = id.clone();
    let cleanup_id = id.clone();
    let focus_id = id.clone();
    cleanup(move || state.row_unmounted(&cleanup_id));
    let resting_y = computed(move || {
        state
            .layout
            .with(|l| l.resting.get(&top_id).copied().unwrap_or(0.))
    });
    let tile_model = computed(move || CardTileModel::build(ui, card.get()));
    let entry = state.take_entry(&id);
    let tile = dynamic(move || card_tile_from_model(ui, tile_model.get()));
    let content = if let Some(kind) = entry {
        let entering = signal(true);
        let entry_scale = animated_value(if kind == EntryKind::PopIn {
            transitions::ENTRY_SCALE_FROM
        } else {
            1.0
        });
        let entry_opacity = animated_value(0.0);
        let ends = match kind {
            EntryKind::PopIn => [
                motion::spring(entry_scale, 1.0, transitions::entry_spring()),
                motion::timing(
                    entry_opacity,
                    1.0,
                    transitions::ENTRY_OPACITY_MS,
                    Easing::EaseOut,
                ),
            ],
            EntryKind::CompactFadeIn => [
                None,
                motion::timing(
                    entry_opacity,
                    1.0,
                    transitions::COMPACT_FADE_MS,
                    Easing::EaseOut,
                ),
            ],
        };
        motion::when_done(ends, move || {
            let _ = entering.try_set(false);
        });
        let initial_epoch = state.motion_epoch.get_untracked();
        effect(move || {
            let epoch = state.motion_epoch.get();
            if (epoch != initial_epoch || motion::reduced()) && entering.get_untracked() {
                entry_scale.set(1.0);
                entry_opacity.set(1.0);
                entering.set(false);
            }
        });
        view()
            .w_full()
            .bind_scale(&entry_scale)
            .bind_opacity(&entry_opacity)
            .child(tile)
            .into_element()
    } else {
        tile.into_element()
    };
    let was_overlay = stored(false);
    let prev_rest = stored(None::<f32>);
    let prev_delta = stored(0f32);
    let prev_epoch = stored(0u64);
    effect(move || {
        let _phase = state.phase.get();
        let lifted = state.lifted.get();
        let ghost_active = state.is_ghost_active(&position_id);
        let epoch = state.motion_epoch.get();
        let (rest, target) = state.layout.with(|l| {
            let rest = l.resting.get(&position_id).copied().unwrap_or(0.);
            let target = l.rows.get(&position_id).copied().unwrap_or(rest);
            (rest, target)
        });
        let delta = target - rest;
        if lifted == position_id || ghost_active {
            y.set(delta);
            was_overlay.replace(true);
            prev_rest.replace(Some(rest));
            prev_delta.replace(delta);
            prev_epoch.replace(epoch);
            return;
        }
        let unmasking = was_overlay.replace(false);
        if unmasking || prev_rest.get().is_none() || epoch != prev_epoch.get() || motion::reduced()
        {
            y.set(delta);
            prev_rest.replace(Some(rest));
            prev_delta.replace(delta);
            prev_epoch.replace(epoch);
            return;
        }
        let old_rest = prev_rest.get().unwrap();
        let old_delta = prev_delta.get();
        let shift = old_rest - rest;
        prev_rest.replace(Some(rest));
        prev_delta.replace(delta);
        prev_epoch.replace(epoch);
        if shift.abs() < 0.5 && (delta - old_delta).abs() < 0.5 {
            return;
        }
        motion::shift(y, shift);
        let _ = motion::spring(y, delta, motion::settle_spring());
    });
    view()
        .absolute()
        .left(px(0.))
        .top(move || px(resting_y.get()))
        .w_full()
        .element_ref(&reference)
        .bind_translate_y(&y)
        .rounded(px(RADIUS))
        .focusable(true)
        .on_focus(move || state.focused.set(focus_id.clone()))
        .on_blur(move || state.focused.set(String::new()))
        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
        .label(move || {
            let c = card.get();
            let ui_merging = ui.merging_cards.with(|ids| ids.contains(&c.id));
            let (merging, replies) = ui.index.with(|idx| {
                let base_merging = idx.card_status.get(&c.id).is_some_and(|st| st.is_merging);
                let merging = base_merging || (c.active() && !c.resolved() && ui_merging);
                (merging, idx.reply_count(&c.id))
            });
            let count = ui
                .reminder_counts
                .with(|all| all.get(&c.id).map(|n| n.total).unwrap_or(0));
            let mut label = format!("Card #{} {}", c.number, c.title);
            if merging {
                label.push_str(" · Merging");
            }
            if replies > 0 {
                label.push_str(&format!(
                    " · {} {}",
                    replies,
                    if replies == 1 { "reply" } else { "replies" }
                ));
            }
            if count > 0 {
                label.push_str(&format!(
                    " · {} notification{}",
                    count,
                    if count == 1 { "" } else { "s" }
                ));
            }
            label
        })
        .opacity(move || {
            if state.is_ghost_active(&hidden_id) {
                0.0_f32
            } else {
                1.0_f32
            }
        })
        .pan_values(Some(&state.x), Some(&state.y))
        .pan_enabled(move || {
            let phase = state.phase.get();
            let pending = ui.is_method_pending("card.move");
            state
                .lifted
                .with(|lifted| super::drag::is_row_pan_enabled(phase, lifted, &pan_id, pending))
        })
        .cursor_pointer()
        .on_layout(move |b: &Bounds<Points>| state.height(&layout_id, b.size.height.0))
        .on_press_down(move |e: &PressEvent| state.down(e))
        .on_pan_start(move |e: &PanEvent| state.start(card.get_untracked(), reference, e))
        .on_pan_end(move |e: &PanEvent| state.end(*e))
        .on_press(move || {
            if state.phase.get_untracked() == 0 {
                ui.open_card(press_id.clone());
            }
        })
        .on_context_menu(move |e: &nsh_core::ContextMenuEvent| {
            crate::card_actions::menu(ui, card.get_untracked(), e.page_position)
        })
        .child(content)
}
