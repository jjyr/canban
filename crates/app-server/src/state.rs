//! Small invalidation cursors; full state is built only for a changed cursor.
use super::*;
use std::time::Instant;

impl AppServer {
    fn state_cursor(&self) -> Value {
        // total_changes catches writes from internal workers as well as public mutations.
        // Rolled-back/no-op writes may invalidate conservatively, never hide a change.
        let changes = self.store.lock().unwrap().db().conn.total_changes();
        let observed_at = self.inventory.lock().unwrap().last_at;
        json!([
            self.identity,
            self.state_instance,
            changes,
            self.state_revision.load(Ordering::SeqCst),
            observed_at
        ])
    }

    pub(super) fn state_response(&self, p: &Value, include_snapshot: bool) -> Result<Value> {
        let wait = Duration::from_millis(p["wait_ms"].as_u64().unwrap_or(0).min(15_000));
        let until = Instant::now() + wait;
        let revision = loop {
            let revision = self.state_cursor();
            if p["after"] != revision || self.stopped() || Instant::now() >= until {
                break revision;
            }
            // Never hold the database while waiting. Bounded rechecks also cover
            // worker commits without emit and notifications racing with wait setup.
            let guard = self.state_signal.0.lock().unwrap();
            drop(
                self.state_signal
                    .1
                    .wait_timeout(
                        guard,
                        until
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(250)),
                    )
                    .unwrap(),
            );
        };
        let changed = p["after"] != revision;
        let mut response = json!({"revision":revision,"changed":changed,
            "server_id":self.identity,"github_revision":self.github_revision.load(Ordering::SeqCst)});
        if include_snapshot && changed {
            // The cursor precedes the read. A concurrent write can cause one extra
            // refresh, but an old snapshot can never acknowledge a newer commit.
            let snapshot = self.snapshot()?;
            let mut span = canban_profiler::Span::new("state.encode");
            span.counts(snapshot.events.len(), 0);
            response["snapshot"] = serde_json::to_value(snapshot)?;
            response["config"] = serde_json::to_value(self.config())?;
            response["home"] = json!(self.home);
            response["hostname"] = json!(network::hostname_string());
        }
        Ok(response)
    }
}

/// Retain only small synchronization metadata, never a second JSON history tree.
pub(super) fn state_marker(state: &Value) -> Value {
    json!({"revision":state["revision"],"github_revision":state["github_revision"]})
}
