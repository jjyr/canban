//! Desktop drafts are independent of Cards, workflow and Timeline history.
use super::*;

impl AppServer {
    pub(super) fn draft_call(&self, method: &str, p: Value) -> Result<Value> {
        ensure!(p["execution"].is_null(), "Drafts require a desktop user");
        if method == "draft.list" {
            let store = self.store.lock().unwrap();
            let mut q = store.db().conn.prepare(
                "SELECT id,revision,updated_at,data FROM editor_drafts WHERE id NOT LIKE 'review-draft:%' ORDER BY updated_at DESC,id",
            )?;
            let rows = q
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, u64>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok(Value::Array(rows.into_iter().map(|(id,revision,updated_at,data)| {
                Ok(json!({"id":id,"revision":revision,"updated_at":updated_at,"data":serde_json::from_str::<Value>(&data)?}))
            }).collect::<Result<Vec<_>>>()?));
        }
        ensure!(method == "draft.save", "Unknown draft operation: {method}");
        let id = strp(&p, "id")?;
        ensure!(
            (id.starts_with("new-card:") || id.starts_with("card:")) && id.len() <= 256,
            "Invalid draft identity"
        );
        let expected = p["revision"]
            .as_u64()
            .context("Draft revision is required")?;
        let data = &p["data"];
        ensure!(
            data.is_object() || data.is_null(),
            "Draft must be an object or null"
        );
        let bytes = serde_json::to_string(data)?;
        ensure!(bytes.len() <= 8 * 1024 * 1024, "Draft exceeds 8 MiB");
        self.store.lock().unwrap().write(|db| {
            let previous = db.conn.query_row("SELECT revision,data FROM editor_drafts WHERE id=?1", [id], |r| Ok((r.get::<_, u64>(0)?,r.get::<_, String>(1)?))).optional()?;
            let revision = previous.as_ref().map(|v| v.0).unwrap_or(0);
            // Identical retries are safe even if the preceding response was lost.
            if previous.as_ref().is_some_and(|(_, value)| value == &bytes) {
                return Ok(json!({"revision":revision}));
            }
            ensure!(revision == expected, "draft_conflict: another window changed this draft; local input is retained");
            let next = revision + 1;
            // Keep tombstones so an old save cannot resurrect a discarded draft.
            db.conn.execute("INSERT INTO editor_drafts(id,revision,updated_at,data) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,updated_at=excluded.updated_at,data=excluded.data", params![id,next,self.now_ms(),bytes])?;
            Ok(json!({"revision":next}))
        })
    }
}
