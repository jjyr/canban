//! Stable column and Card owners; only animated offsets change during a drag.
use super::drag::{BoardDrag, settle};
use crate::{components::card_tile, drafts::*, presenter::*};
use canban_types::{Card, Stage};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
pub(super) fn column(ui: Ui, state: BoardDrag, stage: Stage) -> impl IntoElement {
    let id = stage.id.clone();
    let row_stage = id.clone();
    let height_stage = id.clone();
    let empty_stage = id.clone();
    let count_stage = id.clone();
    let viewport = element_ref();
    let scroll_ref = element_ref();
    state.register(&id, viewport, scroll_ref);
    let key = format!("board:{}:{id}", ui.project.get_untracked());
    let position = scroll_position(ui, &key);
    let explanation = if stage.execute {
        stage
            .profile
            .as_ref()
            .map(|p| format!("Workflow Agent: @{p}"))
            .unwrap_or_else(|| "Workflow Agent: global default".into())
    } else {
        "No automatic Agent execution in this stage".into()
    };
    let gap_y = animated_value(0.);
    let gap_stage = id.clone();
    effect(move || {
        if let Some((s, y, _)) = &state.layout.get().gap
            && s == &gap_stage
        {
            let _ = settle(gap_y, *y);
        }
    });
    let gap_visible_stage = id.clone();
    let gap_height_stage = id.clone();
    let list_stage = id.clone();
    view()
        .w(px(260.))
        .flex_1()
        .min_w(px(220.))
        .min_h(px(0.))
        .gap_3()
        .child(
            view()
                .flex_row()
                .justify_between()
                .items_center()
                .tooltip(nsh_base::tooltip().child(label(explanation)))
                .child(label(stage.name).font_weight(FontWeight::SEMIBOLD))
                .child(badge(move || {
                    state.cards.with(|all| {
                        all.iter()
                            .filter(|c| c.stage == count_stage)
                            .count()
                            .to_string()
                    })
                })),
        )
        .child(
            view()
                .element_ref(&viewport)
                .flex_1()
                .min_h(px(0.))
                .relative()
                .on_layout(move |_: &Bounds<Points>| state.dirty())
                .child(
                    scroll()
                        .element_ref(&scroll_ref)
                        .content_offset(position)
                        .flex_1()
                        .on_scroll(move |e: &ScrollEvent| {
                            state.scrolled(&id, e.offset.y.0);
                            capture_scroll(ui, key.clone(), e);
                        })
                        .child(
                            view()
                                .relative()
                                .w_full()
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
                                                l.gap.as_ref().is_some_and(|(s, _, _)| {
                                                    s == &gap_visible_stage
                                                })
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
                                    move || {
                                        let mut cards = state.cards.with(|all| {
                                            all.iter()
                                                .filter(|c| c.stage == list_stage)
                                                .cloned()
                                                .collect::<Vec<_>>()
                                        });
                                        cards.sort_by(Card::board_cmp);
                                        cards
                                    },
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
                        muted("No cards")
                            .pointer_events(PointerEvents::None)
                            .absolute()
                            .top(px(0.))
                            .left(px(0.))
                    },
                )),
        )
}
fn row(ui: Ui, state: BoardDrag, card: ReadSignal<Card>) -> impl IntoElement {
    let reference = element_ref();
    let y = animated_value(0.);
    let id = card.get_untracked().id;
    let position_id = id.clone();
    let hidden_id = id.clone();
    let press_id = id.clone();
    let layout_id = id.clone();
    let last_target = stored(None::<f32>);
    effect(move || {
        let target = state.layout.with(|l| l.rows.get(&position_id).copied());
        let lifted = state.lifted.get();
        if lifted == position_id {
            return;
        }
        if let Some(target) = target {
            let old = last_target.replace(Some(target));
            if old.is_some_and(|old| (old - target).abs() < 0.1) {
                return;
            }
            if old.is_some() && state.phase.get_untracked() != 0 {
                let _ = settle(y, target);
            } else {
                y.set(target);
            }
        }
    });
    view()
        .absolute()
        .left(px(0.))
        .top(px(0.))
        .w_full()
        .element_ref(&reference)
        .bind_translate_y(&y)
        .rounded(px(RADIUS))
        .focusable(true)
        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
        .label(move || {
            let c = card.get();
            let count = ui
                .reminder_counts
                .with(|all| all.get(&c.id).map(|n| n.total).unwrap_or(0));
            if count > 0 {
                format!(
                    "Card #{} {} · {} notification{}",
                    c.number,
                    c.title,
                    count,
                    if count == 1 { "" } else { "s" }
                )
            } else {
                format!("Card #{} {}", c.number, c.title)
            }
        })
        .opacity(move || {
            if state.lifted.get() == hidden_id {
                0.0_f32
            } else {
                1.0_f32
            }
        })
        .pan_values(Some(&state.x), Some(&state.y))
        .pan_enabled(move || state.phase.get() != 3)
        .on_layout(move |b: &Bounds<Points>| state.height(&layout_id, b.size.height.0))
        .on_press_down(move |e: &PressEvent| state.down(e))
        .on_pan_start(move |_: &PanEvent| state.start(card.get_untracked(), reference))
        .on_pan_end(move |e: &PanEvent| state.end(*e))
        .on_press(move || {
            if state.phase.get_untracked() == 0 {
                ui.open_card(press_id.clone());
            }
        })
        .on_context_menu(move |e: &nsh_core::ContextMenuEvent| {
            crate::card_actions::menu(ui, card.get_untracked(), e.page_position)
        })
        .child(dynamic(move || {
            let _ = state.epoch.get();
            card_tile(ui, card.get())
        }))
}
