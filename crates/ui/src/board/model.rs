//! Pure Board geometry. Pointer movement does not mutate the product snapshot.
use canban_types::Card;
use std::collections::BTreeMap;
pub const GAP: f32 = 12.;
pub const DEFAULT_HEIGHT: f32 = 112.;
const ORDER_GAP: i64 = 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingMove {
    pub card_id: String,
    pub base_revision: u64,
    pub target_stage: String,
    pub before_card_id: Option<String>,
}
/// Optimistically projects `pending` onto `cards` using the same sparse rank
/// rules as `canban_kanban::order::place` so visual order matches server commit.
pub fn apply_pending_move(cards: &mut [Card], pending: &PendingMove) -> bool {
    if pending.before_card_id.as_deref() == Some(pending.card_id.as_str()) {
        return false;
    }
    let Some(card_idx) = cards
        .iter()
        .position(|c| c.id == pending.card_id && c.active())
    else {
        return false;
    };
    let project_id = cards[card_idx].project_id.clone();
    let same_stage = cards[card_idx].stage == pending.target_stage;
    let mut peer_indices: Vec<usize> = cards
        .iter()
        .enumerate()
        .filter(|(i, c)| {
            c.active()
                && c.project_id == project_id
                && c.stage == pending.target_stage
                && *i != card_idx
        })
        .map(|(i, _)| i)
        .collect();
    peer_indices.sort_by(|&a, &b| cards[a].board_cmp(&cards[b]));
    let insert_at = if let Some(before_id) = pending.before_card_id.as_deref() {
        let Some(pos) = peer_indices.iter().position(|&i| cards[i].id == before_id) else {
            return false;
        };
        pos
    } else {
        peer_indices.len()
    };
    if same_stage {
        let mut current_indices = peer_indices.clone();
        current_indices.push(card_idx);
        current_indices.sort_by(|&a, &b| cards[a].board_cmp(&cards[b]));
        let mut desired = peer_indices.clone();
        desired.insert(insert_at, card_idx);
        if current_indices == desired {
            return true;
        }
    } else {
        cards[card_idx].stage = pending.target_stage.clone();
        cards[card_idx].board_order = None;
        cards[card_idx].attention = None;
        cards[card_idx].human_gate = None;
    }
    let calc_rank = |cards: &[Card], peers: &[usize]| -> Option<i64> {
        let left = insert_at
            .checked_sub(1)
            .map(|i| cards[peers[i]].board_rank());
        let right = peers.get(insert_at).map(|&i| cards[i].board_rank());
        match (left, right) {
            (None, None) => Some(0),
            (None, Some(r)) => r.checked_sub(ORDER_GAP),
            (Some(l), None) => l.checked_add(ORDER_GAP),
            (Some(l), Some(r)) => r
                .checked_sub(l)
                .filter(|gap| *gap > 1)
                .map(|gap| l + gap / 2),
        }
    };
    let rank = if let Some(rank) = calc_rank(cards, &peer_indices) {
        rank
    } else {
        for (i, &peer_idx) in peer_indices.iter().enumerate() {
            cards[peer_idx].board_order = Some((i as i64) * ORDER_GAP);
        }
        calc_rank(cards, &peer_indices).unwrap_or(0)
    };
    cards[card_idx].board_order = Some(rank);
    true
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub stage: String,
    pub index: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub index: usize,
    pub top: f32,
    pub bottom: f32,
}
#[derive(Clone, Default, PartialEq)]
pub struct Layout {
    /// Resting top offsets (when idle, with no Card lifted).
    /// Used directly as layout `top` so NSH scroll culling and measurements
    /// operate on real row coordinates rather than stacking all rows at `y = 0`.
    pub resting: BTreeMap<String, f32>,
    pub columns: BTreeMap<String, Vec<Row>>,
    /// Target visual Y positions (equals `resting` when idle; includes peer
    /// displacement and gap spacing while dragging).
    pub rows: BTreeMap<String, f32>,
    pub heights: BTreeMap<String, f32>,
    pub gap: Option<(String, f32, f32)>,
}
pub fn cards<'a>(all: &'a [Card], stage: &str, omit: &str) -> Vec<&'a Card> {
    let mut result: Vec<_> = all
        .iter()
        .filter(|c| c.stage == stage && c.id != omit && c.active())
        .collect();
    result.sort_by(|a, b| a.board_cmp(b));
    result
}
#[cfg(test)]
pub fn layout(
    all: &[Card],
    stages: &[String],
    heights: &BTreeMap<String, f32>,
    dragged: &str,
    slot: Option<&Slot>,
) -> Layout {
    layout_ordered(all, stages, &order(all), heights, dragged, slot)
}
pub type Order = BTreeMap<String, Vec<usize>>;
pub fn order(all: &[Card]) -> Order {
    let mut columns = Order::new();
    for (i, card) in all.iter().enumerate().filter(|(_, c)| c.active()) {
        columns.entry(card.stage.clone()).or_default().push(i);
    }
    for list in columns.values_mut() {
        list.sort_by(|&a, &b| all[a].board_cmp(&all[b]));
    }
    columns
}
pub fn layout_ordered(
    all: &[Card],
    stages: &[String],
    columns: &Order,
    heights: &BTreeMap<String, f32>,
    dragged: &str,
    slot: Option<&Slot>,
) -> Layout {
    let height = |c: &Card| *heights.get(&c.id).unwrap_or(&DEFAULT_HEIGHT);
    let drag_height = *heights.get(dragged).unwrap_or(&DEFAULT_HEIGHT);
    let mut result = Layout::default();
    for stage in stages {
        let list = columns
            .get(stage.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut rest_y = 0.;
        for &i in list {
            let c = &all[i];
            result.resting.insert(c.id.clone(), rest_y);
            rest_y += height(c) + GAP;
        }
        let peers: Vec<_> = list
            .iter()
            .copied()
            .filter(|&i| all[i].id != dragged)
            .collect();
        let mut y = 0.;
        for i in 0..=peers.len() {
            if slot.is_some_and(|s| &s.stage == stage && s.index.min(peers.len()) == i) {
                result.gap = Some((stage.clone(), y, drag_height));
                y += drag_height + GAP;
            }
            if let Some(&i) = peers.get(i) {
                let c = &all[i];
                result.rows.insert(c.id.clone(), y);
                y += height(c) + GAP;
            }
        }
        result.heights.insert(stage.clone(), (y - GAP).max(0.));
        result.columns.insert(
            stage.clone(),
            list.iter()
                .copied()
                .filter(|&i| all[i].id != dragged)
                .map(|i| {
                    let c = &all[i];
                    let rest = result.resting[&c.id];
                    let target = result.rows.get(&c.id).copied().unwrap_or(rest);
                    Row {
                        index: i,
                        top: rest.min(target),
                        bottom: rest.max(target) + height(c),
                    }
                })
                .collect(),
        );
    }
    result
}
/// Bound native owners to the viewport and one screen of overscan. Both resting
/// and displaced bounds participate so animated peers cannot disappear mid-drag.
pub fn visible(rows: &[Row], offset: f32, height: f32) -> &[Row] {
    let overscan = height.max(DEFAULT_HEIGHT * 2.);
    let start = (offset - overscan).max(0.);
    let end = offset + height + overscan;
    let first = rows.partition_point(|r| r.bottom < start);
    let last = rows.partition_point(|r| r.top <= end);
    &rows[first.min(last)..last]
}
/// Hit resting midpoints, not animated frames. A small dead band prevents chatter.
pub fn insertion(
    all: &[Card],
    stage: &str,
    dragged: &str,
    heights: &BTreeMap<String, f32>,
    y: f32,
    previous: Option<&Slot>,
) -> usize {
    let list = cards(all, stage, dragged);
    let mut top = 0.;
    let mut result = 0;
    let old = previous.filter(|s| s.stage == stage).map(|s| s.index);
    for (i, c) in list.iter().enumerate() {
        let h = *heights.get(&c.id).unwrap_or(&DEFAULT_HEIGHT);
        let hysteresis = match old {
            Some(p) if p <= i => 5.,
            Some(_) => -5.,
            None => 0.,
        };
        if y < top + h / 2. + hysteresis {
            break;
        }
        result = i + 1;
        top += h + GAP;
    }
    result
}
pub fn edge_speed(position: f32, start: f32, length: f32) -> f32 {
    let edge = 40_f32.min(length / 3.);
    if position < start + edge {
        -720. * ((start + edge - position) / edge).clamp(0., 1.)
    } else if position > start + length - edge {
        720. * ((position - (start + length - edge)) / edge).clamp(0., 1.)
    } else {
        0.
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn card(id: &str, stage: &str, n: u64) -> Card {
        serde_json::from_value(serde_json::json!({"id":id,"project_id":"p","number":n,"title":id,"body":"","stage":stage,"revision":1,"created_at":0})).unwrap()
    }
    #[test]
    fn placeholder_pushes_peers_and_source_closes_without_changing_cards() {
        let all = vec![
            card("a", "backlog", 1),
            card("b", "develop", 2),
            card("c", "develop", 3),
        ];
        let h = BTreeMap::from([("a".into(), 80.), ("b".into(), 100.), ("c".into(), 60.)]);
        let l = layout(
            &all,
            &["backlog".into(), "develop".into()],
            &h,
            "a",
            Some(&Slot {
                stage: "develop".into(),
                index: 1,
            }),
        );
        assert_eq!(l.rows["b"], 0.);
        assert_eq!(l.rows["c"], 204.);
        assert_eq!(l.gap, Some(("develop".into(), 112., 80.)));
        assert_eq!(l.heights["backlog"], 0.);
        assert_eq!(all[0].stage, "backlog");
        let empty = layout(
            &all,
            &["empty".into()],
            &h,
            "a",
            Some(&Slot {
                stage: "empty".into(),
                index: 0,
            }),
        );
        assert_eq!(empty.gap, Some(("empty".into(), 0., 80.)));
        assert_eq!(l.resting["a"], 0.);
        assert_eq!(l.resting["b"], 0.);
        assert_eq!(l.resting["c"], 112.);
    }
    #[test]
    fn resting_coordinates_match_idle_rows_and_remain_monotonic_for_large_columns() {
        let all: Vec<_> = (1..=35)
            .map(|n| card(&format!("r-{n}"), "review", n))
            .collect();
        let mut h = BTreeMap::new();
        for n in 1..=35 {
            h.insert(format!("r-{n}"), if n % 3 == 0 { 138. } else { 96. });
        }
        let idle = layout(&all, &["review".into()], &h, "", None);
        assert_eq!(idle.resting, idle.rows);
        let mut prev = -1.0_f32;
        for n in 1..=35 {
            let y = idle.resting[&format!("r-{n}")];
            assert!(y > prev, "row {n} resting y={y} must follow {prev}");
            prev = y;
        }
        // Dragging row 1 to index 30 shifts rows 2..=31 up by (h1 + GAP) relative to resting,
        // while keeping `resting` anchored to pre-drag layout coordinates.
        let dragged = layout(
            &all,
            &["review".into()],
            &h,
            "r-1",
            Some(&Slot {
                stage: "review".into(),
                index: 30,
            }),
        );
        assert_eq!(dragged.resting, idle.resting);
        assert_eq!(dragged.rows["r-2"] - dragged.resting["r-2"], -(96. + GAP));
        assert_eq!(dragged.rows["r-35"] - dragged.resting["r-35"], 0.);
    }
    #[test]
    fn five_thousand_rows_mount_only_the_viewport_and_drag_bounds_stay_ordered() {
        let all: Vec<_> = (1..=5000)
            .map(|n| card(&format!("r-{n}"), "backlog", n))
            .collect();
        let stages = ["backlog".into()];
        for dragged in ["", "r-1", "r-35", "r-4999"] {
            let l = layout(
                &all,
                &stages,
                &BTreeMap::new(),
                dragged,
                (!dragged.is_empty()).then_some(&Slot {
                    stage: "backlog".into(),
                    index: 200,
                }),
            );
            let rows = &l.columns["backlog"];
            assert!(
                rows.windows(2)
                    .all(|w| w[0].top <= w[1].top && w[0].bottom <= w[1].bottom)
            );
            for offset in [0., 250_000., l.heights["backlog"] - 700.] {
                let visible = visible(rows, offset, 700.);
                assert!(!visible.is_empty());
                assert!(
                    visible.len() < 24,
                    "{} native rows at offset {offset}",
                    visible.len()
                );
                assert!(
                    visible
                        .iter()
                        .all(|r| r.bottom >= (offset - 700.).max(0.) && r.top <= offset + 1400.)
                );
            }
        }
    }
    #[test]
    fn midpoint_hysteresis_and_scrolled_coordinates_are_stable() {
        let all = vec![card("a", "develop", 1), card("b", "develop", 2)];
        let h = BTreeMap::new();
        let old = Slot {
            stage: "develop".into(),
            index: 0,
        };
        assert_eq!(insertion(&all, "develop", "", &h, 58., Some(&old)), 0);
        assert_eq!(insertion(&all, "develop", "", &h, 62., Some(&old)), 1);
        assert_eq!(insertion(&all, "develop", "", &h, 240., None), 2);
        assert!(edge_speed(10., 0., 300.) < 0.);
        assert!(edge_speed(290., 0., 300.) > 0.);
        assert_eq!(edge_speed(150., 0., 300.), 0.);
    }
    #[test]
    fn apply_pending_move_matches_server_sparse_order_across_stages_and_reorders() {
        let mut cards = vec![
            card("a", "backlog", 1),
            card("b", "backlog", 2),
            card("c", "develop", 3),
            card("d", "develop", 4),
        ];
        // Move `a` into `develop` before `d` (between `c` at 3072 and `d` at 4096 -> 3584).
        let pending = PendingMove {
            card_id: "a".into(),
            base_revision: 1,
            target_stage: "develop".into(),
            before_card_id: Some("d".into()),
        };
        assert!(apply_pending_move(&mut cards, &pending));
        assert_eq!(cards[0].stage, "develop");
        assert_eq!(cards[0].board_order, Some(3584));
        let ordered: Vec<_> = super::cards(&cards, "develop", "")
            .into_iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ordered, vec!["c", "a", "d"]);

        // Move `d` to top of `develop` before `c` (3072 - 1024 = 2048).
        let reorder = PendingMove {
            card_id: "d".into(),
            base_revision: 1,
            target_stage: "develop".into(),
            before_card_id: Some("c".into()),
        };
        assert!(apply_pending_move(&mut cards, &reorder));
        assert_eq!(cards[3].board_order, Some(2048));
        let ordered2: Vec<_> = super::cards(&cards, "develop", "")
            .into_iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ordered2, vec!["d", "c", "a"]);
    }
}
