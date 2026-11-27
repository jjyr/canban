//! Daily Inbox triage state (`inbox.triage`, `inbox.list`).
use super::*;

const MAX_INBOX_KEY_LEN: usize = 512;
const MAX_BATCH_ITEMS: usize = 500;
const INBOX_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;

fn validate_item_key(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    ensure!(
        !trimmed.is_empty()
            && trimmed.len() <= MAX_INBOX_KEY_LEN
            && !trimmed.chars().any(char::is_control),
        "Invalid inbox item id"
    );
    Ok(trimmed.to_owned())
}

impl AppServer {
    pub(super) fn inbox_call(&self, method: &str, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Inbox operations require a user action"
        );
        match method {
            "inbox.list" => {
                let entries = self.store.lock().unwrap().db().list_inbox_entries()?;
                Ok(json!({ "entries": entries }))
            }
            "inbox.triage" => self.inbox_triage(p),
            _ => bail!("Unknown inbox operation: {method}"),
        }
    }

    fn inbox_triage(&self, p: Value) -> Result<Value> {
        let items = p["items"].as_array().context("items array is required")?;
        ensure!(
            !items.is_empty() && items.len() <= MAX_BATCH_ITEMS,
            "items must contain between 1 and {MAX_BATCH_ITEMS} entries"
        );
        let now_ms = self.now_ms();
        let mut parsed = Vec::with_capacity(items.len());
        for item in items {
            let id = validate_item_key(item["id"].as_str().context("item.id is required")?)?;
            let state = item["state"]
                .as_str()
                .context("item.state is required")?
                .trim();
            ensure!(
                matches!(state, "deferred" | "dismissed" | "pinned_today" | "active"),
                "item.state must be deferred, dismissed, pinned_today, or active"
            );
            let defer_until = item["defer_until"].as_u64();
            if state == "deferred" {
                ensure!(
                    defer_until.is_some_and(|ts| ts > 0),
                    "defer_until is required when state is deferred"
                );
            }
            let day_start_ms = item["day_start_ms"].as_u64();
            let watermark = item["watermark"].as_u64();
            parsed.push((id, state.to_owned(), defer_until, day_start_ms, watermark));
        }

        let (updated, removed, entries) = self.store.lock().unwrap().write(|db| {
            let mut updated_count = 0usize;
            let mut removed_count = 0usize;
            let mut saved = Vec::new();
            for (id, state, defer_until, day_start_ms, watermark) in parsed {
                if state == "active" {
                    if db.delete_inbox_entry(&id)? {
                        removed_count += 1;
                    }
                } else {
                    let entry = InboxEntry {
                        id,
                        state,
                        defer_until,
                        day_start_ms,
                        watermark,
                        updated_at: now_ms,
                    };
                    db.save_inbox_entry(&entry)?;
                    saved.push(entry);
                    updated_count += 1;
                }
            }
            let _ = db.prune_inbox_entries(now_ms.saturating_sub(INBOX_RETENTION_MS));
            Ok((updated_count, removed_count, saved))
        })?;

        self.emit("notifications", None);
        Ok(json!({
            "accepted": true,
            "updated": updated,
            "removed": removed,
            "entries": entries,
        }))
    }
}
