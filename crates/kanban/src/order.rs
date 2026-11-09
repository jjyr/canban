//! Sparse Board ranks; changing presentation order never dispatches an Agent.
use anyhow::{Result, ensure};
use canban_storage::Db;
use canban_types::Card;
const GAP: i64 = 1024;

/// Place `card` before an active peer, or at the end. The caller saves the Card
/// and its revision/event in the same transaction as any rare rank compaction.
pub fn place(db: Db<'_>, card: &mut Card, before: Option<&str>) -> Result<bool> {
    ensure!(
        before != Some(card.id.as_str()),
        "A Card cannot be placed before itself"
    );
    let mut peers: Vec<Card> = db
        .list::<Card>()?
        .into_iter()
        .filter(|c| c.active() && c.project_id == card.project_id && c.stage == card.stage)
        .collect();
    peers.sort_by(Card::board_cmp);
    let old: Vec<_> = peers.iter().map(|c| c.id.clone()).collect();
    peers.retain(|c| c.id != card.id);
    let index = if let Some(id) = before {
        peers.iter().position(|c| c.id == id).ok_or_else(|| {
            anyhow::anyhow!("Drop target changed; refresh the Board and try again")
        })?
    } else {
        peers.len()
    };
    let mut desired: Vec<_> = peers.iter().map(|c| c.id.clone()).collect();
    desired.insert(index, card.id.clone());
    if old == desired {
        return Ok(false);
    }
    let rank = |peers: &[Card]| -> Option<i64> {
        let left = index.checked_sub(1).map(|i| peers[i].board_rank());
        let right = peers.get(index).map(Card::board_rank);
        match (left, right) {
            (None, None) => Some(0),
            (None, Some(r)) => r.checked_sub(GAP),
            (Some(l), None) => l.checked_add(GAP),
            (Some(l), Some(r)) => r
                .checked_sub(l)
                .filter(|gap| *gap > 1)
                .map(|gap| l + gap / 2),
        }
    };
    card.board_order = Some(if let Some(rank) = rank(&peers) {
        rank
    } else {
        // Numeric normalization preserves every peer's logical order and content
        // revision. Only the explicitly moved Card gets an order event/revision.
        for (i, peer) in peers.iter_mut().enumerate() {
            peer.board_order = Some((i as i64) * GAP);
            db.save(peer)?;
        }
        rank(&peers).ok_or_else(|| anyhow::anyhow!("Board order is out of range"))?
    });
    Ok(true)
}
