//! Reading never changes a Card revision, workflow gate or legacy Notice.
use super::*;
impl AppServer {
    pub(super) fn read_card(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Read positions require a user action"
        );
        let card_id = strp(&p, "card_id")?;
        let sequence = p["sequence"].as_u64().context("sequence is required")?;
        let (read, changed) = self.store.lock().unwrap().write(|db| {
            let card: Card = db.get(card_id)?;
            let maximum: u64 = db.conn.query_row(
                "SELECT COALESCE(MAX(number),0) FROM events WHERE card_id=?1",
                [card_id],
                |row| row.get(0),
            )?;
            let saved: Option<String> = db
                .conn
                .query_row(
                    "SELECT data FROM card_reads WHERE id=?1",
                    [card_id],
                    |row| row.get(0),
                )
                .optional()?;
            let old: Option<CardRead> =
                saved.map(|json| serde_json::from_str(&json)).transpose()?;
            let previous = old.as_ref().map(|r| r.sequence).unwrap_or(0);
            let read = CardRead {
                id: card.id.clone(),
                project_id: card.project_id,
                card_id: card.id,
                sequence: previous.max(sequence.min(maximum)),
            };
            let changed = read.sequence > previous;
            if changed {
                db.save(&read)?;
            }
            Ok((read, changed))
        })?;
        if changed {
            self.emit("notifications", Some(card_id.into()));
        }
        Ok(serde_json::to_value(read)?)
    }
    pub(super) fn read_all_cards(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Read positions require a user action"
        );
        let project_filter = p["project_id"].as_str().filter(|s| !s.is_empty());
        let updated = self.store.lock().unwrap().write(|db| {
            let cards = db.list::<Card>()?;
            let mut count = 0u64;
            for card in cards {
                if card.removed_at.is_some() {
                    continue;
                }
                if let Some(pid) = project_filter
                    && card.project_id != pid
                {
                    continue;
                }
                let maximum: u64 = db.conn.query_row(
                    "SELECT COALESCE(MAX(number),0) FROM events WHERE card_id=?1",
                    [&card.id],
                    |row| row.get(0),
                )?;
                if maximum == 0 {
                    continue;
                }
                let saved: Option<String> = db
                    .conn
                    .query_row(
                        "SELECT data FROM card_reads WHERE id=?1",
                        [&card.id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let old: Option<CardRead> =
                    saved.map(|json| serde_json::from_str(&json)).transpose()?;
                let previous = old.as_ref().map(|r| r.sequence).unwrap_or(0);
                if maximum > previous {
                    let read = CardRead {
                        id: card.id.clone(),
                        project_id: card.project_id,
                        card_id: card.id,
                        sequence: maximum,
                    };
                    db.save(&read)?;
                    count += 1;
                }
            }
            Ok(count)
        })?;
        if updated > 0 {
            self.emit("notifications", None);
        }
        Ok(json!({"updated": updated}))
    }
}
