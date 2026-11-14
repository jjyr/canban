use super::*;
impl AppServer {
    pub(super) fn overview_preview(&self, p: Value) -> Result<Value> {
        let snapshot = self.snapshot()?;
        let rows = canban_notifications::overview(&snapshot);
        let row = rows
            .iter()
            .find(|r| r.id == p["row_id"].as_str().unwrap_or(""))
            .context("Overview context no longer exists")?;
        let entry = row
            .terminal
            .as_ref()
            .context("No terminal for this context")?;
        let run = entry
            .agent
            .as_ref()
            .context("No Agent assignment for this context")?;
        ensure!(
            Some(entry.id.as_str()) == p["terminal_id"].as_str(),
            "Overview invocation changed; select the current row again"
        );
        // A finished invocation is durable task context, not a live Terminal.
        let (mut text, source, observed_at) = if run.finished() {
            let path = run
                .output_path
                .as_deref()
                .context("No saved output for this invocation; open the Card for recovery")?;
            let saved = self.read_resource(&row.card.id, path)?;
            let text = saved["body"]
                .as_str()
                .context("No saved execution text")?
                .to_owned();
            (text, "Saved execution output (read-only)", None)
        } else {
            let client = canban_herdr::Client {
                config: self.config(),
            };
            ensure!(
                client.status()?["core"]["id"].as_str() == run.core_id.as_deref(),
                "Original Herdr core unavailable; retained context only"
            );
            let terminal = run.terminal.as_deref().context("No bound terminal")?;
            let screen = client.read(terminal)?;
            let text = screen["text"]
                .as_str()
                .context("No terminal text")?
                .to_owned();
            (text, "Herdr terminal screen (read-only)", Some(now()))
        };
        // Screen preview is read-only and bounded independently of terminal history.
        if text.len() > 32_768 {
            let mut start = text.len() - 32_768;
            while !text.is_char_boundary(start) {
                start += 1;
            }
            text = text[start..].to_owned();
        }
        let current = canban_notifications::overview(&self.snapshot()?);
        ensure!(
            current
                .iter()
                .any(|r| r.id == row.id && r.terminal.as_ref().is_some_and(|t| t.id == entry.id)),
            "Overview invocation changed while reading"
        );
        Ok(
            json!({"text":text,"source":source,"observed_at":observed_at,"terminal_id":entry.id,"terminal":run.terminal,"core_id":run.core_id}),
        )
    }
}
