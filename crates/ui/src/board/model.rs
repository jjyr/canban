//! Pure Board geometry. Pointer movement does not mutate the product snapshot.
use canban_types::Card;
use std::collections::BTreeMap;
pub const GAP: f32 = 12.;
pub const DEFAULT_HEIGHT: f32 = 112.;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub stage: String,
    pub index: usize,
}
#[derive(Clone, Default, PartialEq)]
pub struct Layout {
    /// Resting top offsets (when idle, with no Card lifted).
    /// Used directly as layout `top` so NSH scroll culling and measurements
    /// operate on real row coordinates rather than stacking all rows at `y = 0`.
    pub resting: BTreeMap<String, f32>,
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
pub fn layout(
    all: &[Card],
    stages: &[String],
    heights: &BTreeMap<String, f32>,
    dragged: &str,
    slot: Option<&Slot>,
) -> Layout {
    let height = |c: &Card| *heights.get(&c.id).unwrap_or(&DEFAULT_HEIGHT);
    let drag_height = *heights.get(dragged).unwrap_or(&DEFAULT_HEIGHT);
    let mut result = Layout::default();
    for stage in stages {
        let mut rest_y = 0.;
        for c in cards(all, stage, "") {
            result.resting.insert(c.id.clone(), rest_y);
            rest_y += height(c) + GAP;
        }
        let list = cards(all, stage, dragged);
        let mut y = 0.;
        for i in 0..=list.len() {
            if slot.is_some_and(|s| &s.stage == stage && s.index.min(list.len()) == i) {
                result.gap = Some((stage.clone(), y, drag_height));
                y += drag_height + GAP;
            }
            if let Some(c) = list.get(i) {
                result.rows.insert(c.id.clone(), y);
                y += height(c) + GAP;
            }
        }
        result.heights.insert(stage.clone(), (y - GAP).max(0.));
    }
    result
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
}
