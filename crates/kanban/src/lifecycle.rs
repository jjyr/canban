//! Reversible Card removal shared by local and external Cards.
use anyhow::{Result, ensure};
use canban_storage::Db;
use canban_types::{Attempt, Card, now};
use serde_json::json;
pub fn remove(db: Db<'_>, card: &mut Card) -> Result<()> {
    ensure!(
        !db.list::<Attempt>()?.iter().any(|a| a.card_id == card.id
            && a.terminal.is_some()
            && a.observed.as_ref().is_none_or(|s| s["state"] != "finished")),
        "End active Card Agents before removing this Card"
    );
    ensure!(
        !db.list::<Attempt>()?.iter().any(|a| a.card_id == card.id
            && a.terminal.is_none()
            && matches!(a.state.as_str(), "starting" | "unknown")),
        "Resolve uncertain Card executions before removal"
    );
    for mut a in db
        .list::<Attempt>()?
        .into_iter()
        .filter(|a| a.card_id == card.id && a.state == "queued")
    {
        a.state = "cancelled".into();
        db.save(&a)?;
    }
    card.removed_at = Some(now());
    card.revision += 1;
    db.save(card)?;
    db.append(canban_timeline::event(
        &card.project_id,
        &card.id,
        "card.removed",
        "user",
        if card.github_issue().is_some() {
            "Removed from Board; GitHub source unchanged"
        } else {
            "Card deleted; history retained for restoration"
        },
        json!({}),
    ))?;
    Ok(())
}
