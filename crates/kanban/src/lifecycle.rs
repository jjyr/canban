//! Reversible Card removal shared by local and external Cards.
use anyhow::{Result, ensure};
use canban_storage::Db;
use canban_types::{Card, TerminalEntry, now};
use serde_json::json;
pub fn remove(db: Db<'_>, card: &mut Card) -> Result<()> {
    ensure!(
        !db.list::<TerminalEntry>()?.iter().any(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == card.id
                    && a.terminal.is_some()
                    && a.observed.as_ref().is_none_or(|s| s["state"] != "finished")
            })
        }),
        "End active Card Agents before removing this Card"
    );
    ensure!(
        !db.list::<TerminalEntry>()?.iter().any(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == card.id
                    && a.terminal.is_none()
                    && matches!(a.state.as_str(), "starting" | "unknown")
            })
        }),
        "Resolve uncertain Card executions before removal"
    );
    for mut t in db.list::<TerminalEntry>()? {
        let mut changed = false;
        if let Some(a) = &mut t.agent
            && a.card_id == card.id
        {
            if a.state == "queued" {
                a.state = "cancelled".into();
                changed = true;
            }
            if a.pending.take().is_some() {
                changed = true;
            }
        }
        if changed {
            db.save(&t)?;
        }
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
