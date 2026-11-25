//! Small invalidation cursors & broadcast push events; full state is built only for a changed cursor.
use super::*;

#[derive(Clone, Debug)]
pub(crate) enum PushEvent {
    StateChanged {
        seq: u64,
        topic: &'static str,
        card_id: Option<String>,
        github_revision: u64,
    },
    UpdateChanged {
        seq: u64,
        state: Value,
    },
    UtilitiesChanged {
        seq: u64,
        state: Value,
    },
    UsageChanged {
        seq: u64,
        state: Value,
    },
    HerdrInstall {
        seq: u64,
        state: Value,
    },
}

impl AppServer {
    pub(crate) fn fast_state_cursor(&self) -> Value {
        json!([
            self.identity,
            self.state_instance,
            self.db_changes.load(Ordering::SeqCst),
            self.state_revision.load(Ordering::SeqCst),
            self.observed_at.load(Ordering::SeqCst)
        ])
    }

    pub(crate) fn state_cursor(&self) -> Value {
        // total_changes catches writes from internal workers as well as public mutations.
        // Rolled-back/no-op writes may invalidate conservatively, never hide a change.
        let changes = self.store.lock().unwrap().db().conn.total_changes();
        self.db_changes.store(changes, Ordering::SeqCst);
        let observed_at = self.inventory.lock().unwrap().last_at;
        self.observed_at.store(observed_at, Ordering::SeqCst);
        json!([
            self.identity,
            self.state_instance,
            changes,
            self.state_revision.load(Ordering::SeqCst),
            observed_at
        ])
    }

    pub(super) fn state_response(&self, p: &Value) -> Result<Value> {
        let revision = self.state_cursor();
        let changed = p["after"] != revision;
        let mut response = json!({
            "revision": revision,
            "changed": changed,
            "server_id": self.identity,
            "github_revision": self.github_revision.load(Ordering::SeqCst),
        });
        if changed {
            // The cursor precedes the read. A concurrent write can cause one extra
            // refresh, but an old snapshot can never acknowledge a newer commit.
            let snapshot = self.snapshot()?;
            let mut span = canban_profiler::Span::new("state.encode");
            span.counts(snapshot.cards.len(), 0);
            response["snapshot"] = serde_json::to_value(snapshot)?;
            response["config"] = serde_json::to_value(self.config())?;
            response["home"] = json!(self.home);
            response["hostname"] = json!(network::hostname_with(self.exec_runner.as_ref()));
        }
        Ok(response)
    }

    pub(super) fn runtime_state_response(&self, _p: &Value) -> Result<Value> {
        Ok(json!({
            "server_id": self.identity,
            "runtime": self.runtime_state()?,
        }))
    }
}

/// Retain only small synchronization metadata, never a second JSON history tree.
pub(super) fn state_marker(state: &Value) -> Value {
    json!({"revision":state["revision"],"github_revision":state["github_revision"]})
}
