//! Non-blocking Board transition planner and nonlinear motion curves.
use super::model::{self, Layout};
use canban_types::Card;
use nsh_core::prelude::*;
use std::collections::{BTreeMap, HashMap};

pub(super) const ENTRY_SCALE_FROM: f32 = 0.80;
pub(super) const ENTRY_OPACITY_MS: f32 = 150.0;
pub(super) const COMPACT_FADE_MS: f32 = 180.0;
pub(super) const EXIT_FADE_MS: f32 = 190.0;
pub(super) const EXIT_SCALE_TO: f32 = 0.94;
pub(super) const FLIGHT_FADE_MS: f32 = 180.0;
pub(super) const FLIGHT_TIMEOUT_MS: u64 = 450;
pub(super) const EXIT_TIMEOUT_MS: u64 = 320;

/// Critically damped spring for cross-column flights: fast and crisp without
/// overshoot, softening slightly across longer horizontal spans.
pub(super) fn flight_spring(distance: f32) -> Spring {
    let t = (distance / 800.0).clamp(0.0, 1.0);
    let stiffness = 1400.0 - t * 500.0;
    Spring::from_stiffness(stiffness).damping_ratio(1.0)
}

/// Underdamped spring for newly created Cards popping onto the Board ("duang").
pub(super) fn entry_spring() -> Spring {
    Spring::from_stiffness(680.0).damping_ratio(0.50)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EntryKind {
    PopIn,
    CompactFadeIn,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FlightSpec {
    pub id: String,
    pub card: Card,
    pub origin_x: f32,
    pub origin_y: f32,
    pub target_x: f32,
    pub target_y: f32,
    pub width: f32,
    pub from_opacity: f32,
    pub to_opacity: f32,
    pub target_stage: String,
    pub exiting: bool,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ExitSpec {
    pub id: String,
    pub card: Card,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ColumnMetric {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub offset: f32,
}

#[derive(Default)]
pub(super) struct TransitionPlan {
    pub flights: Vec<FlightSpec>,
    pub exits: Vec<ExitSpec>,
    pub entries: BTreeMap<String, EntryKind>,
    pub flip_offsets: BTreeMap<String, f32>,
}

fn clamp_to_edge(raw_y: f32, view_top: f32, view_h: f32, card_h: f32) -> f32 {
    if raw_y + card_h <= view_top {
        view_top - card_h * 0.35
    } else {
        view_top + view_h - card_h * 0.65
    }
}

fn is_vertically_visible(raw_y: f32, card_h: f32, view_top: f32, view_h: f32) -> bool {
    view_h > 0.0 && raw_y + card_h > view_top && raw_y < view_top + view_h
}

fn is_horizontally_visible(x: f32, w: f32, root_w: f32) -> bool {
    root_w > 0.0 && w > 0.0 && x + w > 0.0 && x < root_w
}

/// Computes non-blocking Board transitions by diffing previous vs. next Cards
/// and resting layout coordinates against measured column viewports.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan(
    old_cards: &[Card],
    new_cards: &[Card],
    old_layout: &Layout,
    new_layout: &Layout,
    heights: &BTreeMap<String, f32>,
    columns: &BTreeMap<String, ColumnMetric>,
    root_w: f32,
    active_flights: &[FlightSpec],
    next_gen: &mut u64,
) -> TransitionPlan {
    let mut result = TransitionPlan::default();
    let old_by_id: HashMap<&str, &Card> = old_cards.iter().map(|c| (c.id.as_str(), c)).collect();
    let new_by_id: HashMap<&str, &Card> = new_cards.iter().map(|c| (c.id.as_str(), c)).collect();
    let active_by_id: HashMap<&str, &FlightSpec> =
        active_flights.iter().map(|f| (f.id.as_str(), f)).collect();
    let has_geometry = root_w > 0.0 && !columns.is_empty();
    let compact = columns.len() <= 1;

    let mut take_gen = || {
        *next_gen = next_gen.wrapping_add(1);
        *next_gen
    };

    for new_card in new_cards {
        let card_h = heights
            .get(&new_card.id)
            .copied()
            .unwrap_or(model::DEFAULT_HEIGHT);
        if let Some(old_card) = old_by_id.get(new_card.id.as_str()) {
            let existing_flight = active_by_id.get(new_card.id.as_str()).copied();
            if old_card.stage != new_card.stage {
                if !has_geometry {
                    continue;
                }
                let old_col = columns.get(&old_card.stage);
                let new_col = columns.get(&new_card.stage);
                let old_rest = old_layout.resting.get(&new_card.id).copied();
                let new_rest = new_layout.resting.get(&new_card.id).copied();

                if !compact
                    && let (Some(oc), Some(nc), Some(oy), Some(ny)) =
                        (old_col, new_col, old_rest, new_rest)
                    && oc.w > 0.0
                    && oc.h > 0.0
                    && nc.w > 0.0
                    && nc.h > 0.0
                {
                    let raw_from_y = oc.y + (oy - oc.offset);
                    let raw_to_y = nc.y + (ny - nc.offset);
                    let from_vis_y = is_vertically_visible(raw_from_y, card_h, oc.y, oc.h);
                    let to_vis_y = is_vertically_visible(raw_to_y, card_h, nc.y, nc.h);
                    let from_visible = from_vis_y && is_horizontally_visible(oc.x, oc.w, root_w);
                    let to_visible = to_vis_y && is_horizontally_visible(nc.x, nc.w, root_w);

                    if let Some(prev) = existing_flight {
                        if to_visible || prev.to_opacity > 0.0 {
                            let target_y = if to_vis_y {
                                raw_to_y
                            } else {
                                clamp_to_edge(raw_to_y, nc.y, nc.h, card_h)
                            };
                            let to_opacity = if to_visible { 1.0 } else { 0.0 };
                            let mut updated = prev.clone();
                            updated.card = new_card.clone();
                            updated.target_x = nc.x;
                            updated.target_y = target_y;
                            updated.width = nc.w;
                            updated.to_opacity = to_opacity;
                            updated.target_stage = new_card.stage.clone();
                            updated.exiting = false;
                            updated.generation = take_gen();
                            result.flights.push(updated);
                        }
                    } else if from_visible || to_visible {
                        let origin_y = if from_vis_y {
                            raw_from_y
                        } else {
                            clamp_to_edge(raw_from_y, oc.y, oc.h, card_h)
                        };
                        let target_y = if to_vis_y {
                            raw_to_y
                        } else {
                            clamp_to_edge(raw_to_y, nc.y, nc.h, card_h)
                        };
                        result.flights.push(FlightSpec {
                            id: new_card.id.clone(),
                            card: new_card.clone(),
                            origin_x: oc.x,
                            origin_y,
                            target_x: nc.x,
                            target_y,
                            width: nc.w,
                            from_opacity: if from_visible { 1.0 } else { 0.25 },
                            to_opacity: if to_visible { 1.0 } else { 0.0 },
                            target_stage: new_card.stage.clone(),
                            exiting: false,
                            generation: take_gen(),
                        });
                    }
                } else {
                    // Compact single-column mode or unmounted source/destination column:
                    // avoid fake cross-column flight; lightly fade in/out in place.
                    if let (Some(nc), Some(ny)) = (new_col, new_rest)
                        && is_horizontally_visible(nc.x, nc.w, root_w)
                        && is_vertically_visible(nc.y + (ny - nc.offset), card_h, nc.y, nc.h)
                    {
                        result
                            .entries
                            .insert(new_card.id.clone(), EntryKind::CompactFadeIn);
                    } else if let (Some(oc), Some(oy)) = (old_col, old_rest) {
                        let raw_y = oc.y + (oy - oc.offset);
                        if is_horizontally_visible(oc.x, oc.w, root_w)
                            && is_vertically_visible(raw_y, card_h, oc.y, oc.h)
                        {
                            result.exits.push(ExitSpec {
                                id: new_card.id.clone(),
                                card: (*old_card).clone(),
                                x: oc.x,
                                y: raw_y,
                                width: oc.w,
                                generation: take_gen(),
                            });
                        }
                    }
                }
            } else if let Some(prev) = existing_flight {
                // Card was already mid-flight into this column; keep its flight live
                // and retarget if its destination slot shifted.
                if let (Some(nc), Some(ny)) = (
                    columns.get(&new_card.stage),
                    new_layout.resting.get(&new_card.id).copied(),
                ) {
                    let raw_to_y = nc.y + (ny - nc.offset);
                    let to_vis_y = is_vertically_visible(raw_to_y, card_h, nc.y, nc.h);
                    let to_visible = to_vis_y && is_horizontally_visible(nc.x, nc.w, root_w);
                    let target_y = if to_vis_y {
                        raw_to_y
                    } else {
                        clamp_to_edge(raw_to_y, nc.y, nc.h, card_h)
                    };
                    let to_opacity = if to_visible { 1.0 } else { 0.0 };
                    let geometry_changed = (prev.target_x - nc.x).abs() > 0.5
                        || (prev.target_y - target_y).abs() > 0.5
                        || (prev.to_opacity - to_opacity).abs() > 0.01;
                    let mut updated = prev.clone();
                    updated.card = new_card.clone();
                    if geometry_changed {
                        updated.target_x = nc.x;
                        updated.target_y = target_y;
                        updated.width = nc.w;
                        updated.to_opacity = to_opacity;
                        updated.generation = take_gen();
                    }
                    result.flights.push(updated);
                } else {
                    let mut updated = prev.clone();
                    updated.card = new_card.clone();
                    result.flights.push(updated);
                }
            } else if let (Some(&oy), Some(&ny)) = (
                old_layout.resting.get(&new_card.id),
                new_layout.resting.get(&new_card.id),
            ) && (oy - ny).abs() > 0.5
            {
                result.flip_offsets.insert(new_card.id.clone(), oy - ny);
            }
        } else if has_geometry
            && let (Some(nc), Some(ny)) = (
                columns.get(&new_card.stage),
                new_layout.resting.get(&new_card.id).copied(),
            )
            && is_horizontally_visible(nc.x, nc.w, root_w)
            && is_vertically_visible(nc.y + (ny - nc.offset), card_h, nc.y, nc.h)
        {
            result.entries.insert(new_card.id.clone(), EntryKind::PopIn);
        }
    }

    for old_card in old_cards {
        if new_by_id.contains_key(old_card.id.as_str()) {
            continue;
        }
        if let Some(prev) = active_by_id.get(old_card.id.as_str()) {
            let mut fading = (*prev).clone();
            fading.exiting = true;
            fading.to_opacity = 0.0;
            fading.generation = take_gen();
            result.flights.push(fading);
            continue;
        }
        if !has_geometry {
            continue;
        }
        let card_h = heights
            .get(&old_card.id)
            .copied()
            .unwrap_or(model::DEFAULT_HEIGHT);
        if let (Some(oc), Some(oy)) = (
            columns.get(&old_card.stage),
            old_layout.resting.get(&old_card.id).copied(),
        ) {
            let raw_y = oc.y + (oy - oc.offset);
            if is_horizontally_visible(oc.x, oc.w, root_w)
                && is_vertically_visible(raw_y, card_h, oc.y, oc.h)
            {
                result.exits.push(ExitSpec {
                    id: old_card.id.clone(),
                    card: old_card.clone(),
                    x: oc.x,
                    y: raw_y,
                    width: oc.w,
                    generation: take_gen(),
                });
            }
        }
    }

    // Keep any already-fading mid-air flights that were not re-added in new_cards.
    for prev in active_flights {
        if prev.exiting
            && !new_by_id.contains_key(prev.id.as_str())
            && !result.flights.iter().any(|f| f.id == prev.id)
        {
            result.flights.push(prev.clone());
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, stage: &str, n: u64) -> Card {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "project_id": "p",
            "number": n,
            "title": id,
            "body": "",
            "stage": stage,
            "revision": 1,
            "created_at": n
        }))
        .unwrap()
    }

    fn cols() -> BTreeMap<String, ColumnMetric> {
        BTreeMap::from([
            (
                "backlog".into(),
                ColumnMetric {
                    x: 0.0,
                    y: 40.0,
                    w: 280.0,
                    h: 600.0,
                    offset: 0.0,
                },
            ),
            (
                "develop".into(),
                ColumnMetric {
                    x: 292.0,
                    y: 40.0,
                    w: 280.0,
                    h: 600.0,
                    offset: 0.0,
                },
            ),
            (
                "review".into(),
                ColumnMetric {
                    x: 584.0,
                    y: 40.0,
                    w: 280.0,
                    h: 600.0,
                    offset: 0.0,
                },
            ),
        ])
    }

    #[test]
    fn cross_column_move_creates_flight_and_flips_peers() {
        let stages = vec!["backlog".into(), "develop".into(), "review".into()];
        let old_cards = vec![
            card("a", "backlog", 1),
            card("b", "backlog", 2),
            card("c", "develop", 3),
        ];
        let new_cards = vec![
            card("a", "develop", 1),
            card("b", "backlog", 2),
            card("c", "develop", 3),
        ];
        let heights = BTreeMap::new();
        let old_layout = model::layout(&old_cards, &stages, &heights, "", None);
        let new_layout = model::layout(&new_cards, &stages, &heights, "", None);
        let mut next_gen = 0;

        let plan = plan(
            &old_cards,
            &new_cards,
            &old_layout,
            &new_layout,
            &heights,
            &cols(),
            1000.0,
            &[],
            &mut next_gen,
        );

        assert_eq!(plan.flights.len(), 1);
        let f = &plan.flights[0];
        assert_eq!(f.id, "a");
        assert_eq!(f.origin_x, 0.0);
        assert_eq!(f.origin_y, 40.0);
        assert_eq!(f.target_x, 292.0);
        assert_eq!(f.target_y, 40.0);
        assert_eq!(f.from_opacity, 1.0);
        assert_eq!(f.to_opacity, 1.0);
        // `b` moved up from 124 to 0 -> FLIP offset = +124; `c` moved down from 0 to 124 -> -124.
        assert_eq!(plan.flip_offsets.get("b").copied(), Some(124.0));
        assert_eq!(plan.flip_offsets.get("c").copied(), Some(-124.0));
        assert!(plan.entries.is_empty());
        assert!(plan.exits.is_empty());
    }

    #[test]
    fn offscreen_endpoints_skip_or_clamp_flight_cleanly() {
        let stages = vec!["backlog".into(), "develop".into()];
        let mut old_cards = Vec::new();
        let mut new_cards = Vec::new();
        for n in 1..=20 {
            old_cards.push(card(&format!("b-{n}"), "backlog", n));
            new_cards.push(card(&format!("b-{n}"), "backlog", n));
        }
        // Move bottom-most card `b-20` (off-screen at y ~ 2356) to top of `develop` (visible).
        new_cards[19].stage = "develop".into();
        new_cards[19].board_order = Some(-1);
        let heights = BTreeMap::new();
        let old_layout = model::layout(&old_cards, &stages, &heights, "", None);
        let new_layout = model::layout(&new_cards, &stages, &heights, "", None);
        let mut next_gen = 0;

        let one_side = plan(
            &old_cards,
            &new_cards,
            &old_layout,
            &new_layout,
            &heights,
            &cols(),
            1000.0,
            &[],
            &mut next_gen,
        );
        assert_eq!(one_side.flights.len(), 1);
        assert_eq!(one_side.flights[0].from_opacity, 0.25);
        assert_eq!(one_side.flights[0].to_opacity, 1.0);
        assert!(one_side.flights[0].origin_y <= 640.0);

        // Now scroll `develop` deep down too so both source and destination are off-screen.
        let mut scrolled_cols = cols();
        scrolled_cols.get_mut("develop").unwrap().offset = 2000.0;
        let both_offscreen = plan(
            &old_cards,
            &new_cards,
            &old_layout,
            &new_layout,
            &heights,
            &scrolled_cols,
            1000.0,
            &[],
            &mut next_gen,
        );
        assert!(both_offscreen.flights.is_empty());
    }

    #[test]
    fn rapid_retarget_preserves_origin_and_bumps_generation() {
        let stages = vec!["backlog".into(), "develop".into(), "review".into()];
        let c1 = vec![card("a", "backlog", 1)];
        let c2 = vec![card("a", "develop", 1)];
        let c3 = vec![card("a", "review", 1)];
        let heights = BTreeMap::new();
        let l1 = model::layout(&c1, &stages, &heights, "", None);
        let l2 = model::layout(&c2, &stages, &heights, "", None);
        let l3 = model::layout(&c3, &stages, &heights, "", None);
        let mut next_gen = 0;

        let p1 = plan(
            &c1,
            &c2,
            &l1,
            &l2,
            &heights,
            &cols(),
            1000.0,
            &[],
            &mut next_gen,
        );
        assert_eq!(p1.flights[0].origin_x, 0.0);
        assert_eq!(p1.flights[0].target_x, 292.0);
        let g1 = p1.flights[0].generation;

        let p2 = plan(
            &c2,
            &c3,
            &l2,
            &l3,
            &heights,
            &cols(),
            1000.0,
            &p1.flights,
            &mut next_gen,
        );
        assert_eq!(p2.flights.len(), 1);
        assert_eq!(p2.flights[0].origin_x, 0.0);
        assert_eq!(p2.flights[0].target_x, 584.0);
        assert!(p2.flights[0].generation > g1);
    }

    #[test]
    fn compact_mode_uses_fade_instead_of_cross_column_flight() {
        let stages = vec!["backlog".into(), "develop".into()];
        let old_cards = vec![card("a", "backlog", 1)];
        let new_cards = vec![card("a", "develop", 1)];
        let heights = BTreeMap::new();
        let old_layout = model::layout(&old_cards, &stages, &heights, "", None);
        let new_layout = model::layout(&new_cards, &stages, &heights, "", None);
        let compact_cols = BTreeMap::from([(
            "develop".into(),
            ColumnMetric {
                x: 0.0,
                y: 40.0,
                w: 320.0,
                h: 600.0,
                offset: 0.0,
            },
        )]);
        let mut next_gen = 0;

        let plan = plan(
            &old_cards,
            &new_cards,
            &old_layout,
            &new_layout,
            &heights,
            &compact_cols,
            360.0,
            &[],
            &mut next_gen,
        );
        assert!(plan.flights.is_empty());
        assert_eq!(
            plan.entries.get("a").copied(),
            Some(EntryKind::CompactFadeIn)
        );
    }

    #[test]
    fn created_and_archived_cards_produce_pop_in_and_fade_out() {
        let stages = vec!["backlog".into(), "develop".into()];
        let old_cards = vec![card("gone", "backlog", 1), card("stay", "backlog", 2)];
        let new_cards = vec![card("stay", "backlog", 2), card("born", "develop", 3)];
        let heights = BTreeMap::new();
        let old_layout = model::layout(&old_cards, &stages, &heights, "", None);
        let new_layout = model::layout(&new_cards, &stages, &heights, "", None);
        let mut next_gen = 0;

        let plan = plan(
            &old_cards,
            &new_cards,
            &old_layout,
            &new_layout,
            &heights,
            &cols(),
            1000.0,
            &[],
            &mut next_gen,
        );
        assert_eq!(plan.entries.get("born").copied(), Some(EntryKind::PopIn));
        assert_eq!(plan.exits.len(), 1);
        assert_eq!(plan.exits[0].id, "gone");
        assert_eq!(plan.exits[0].x, 0.0);
        assert_eq!(plan.exits[0].y, 40.0);
        assert_eq!(plan.flip_offsets.get("stay").copied(), Some(124.0));
    }
}
