//! Read-your-writes while a background (possibly cached remote) snapshot catches up.
//! Only server-confirmed replies enter this overlay; failed submissions never do.
use canban_types::{Card, Event, Snapshot, TerminalEntry};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn immediate(method: &str) -> bool {
    matches!(
        method,
        "card.create" | "comment.add" | "github.issue.add" | "github.item.add" | "terminal.create"
    )
}

#[derive(Default)]
pub(crate) struct CommittedWrites {
    cards: BTreeMap<String, Card>,
    comments: BTreeMap<String, Event>,
    terminals: BTreeMap<String, (TerminalEntry, String)>,
}

impl CommittedWrites {
    pub(super) fn record(&mut self, method: &str, reply: &Value, host: &str) {
        if method == "card.remove" && reply["removed"] == true {
            if let Some(id) = reply["card_id"].as_str() {
                self.cards.remove(id);
                self.comments.retain(|_, event| event.card_id != id);
            }
            return;
        }
        if !immediate(method) {
            return;
        }
        if method == "terminal.create" {
            if let Ok(terminal) = serde_json::from_value::<TerminalEntry>(reply.clone()) {
                self.terminals
                    .insert(terminal.id.clone(), (terminal, host.into()));
            }
        } else if method == "comment.add" {
            if let Ok(event) = serde_json::from_value::<Event>(reply.clone()) {
                self.comments.insert(event.id.clone(), event);
            }
        } else if let Ok(card) = serde_json::from_value::<Card>(reply.clone()) {
            self.cards.insert(card.id.clone(), card);
        }
    }

    pub(super) fn overlay(&self, snapshot: &mut Snapshot) {
        for (terminal, host) in self.terminals.values() {
            // Never overwrite a newer runtime observation (exit, close or rename).
            if !snapshot.terminals.iter().any(|t| t.id == terminal.id) {
                snapshot.terminals.push(terminal.clone());
            }
            snapshot
                .entity_hosts
                .insert(terminal.id.clone(), host.clone());
        }
        for card in self.cards.values() {
            if let Some(saved) = snapshot.cards.iter_mut().find(|c| c.id == card.id) {
                if saved.revision < card.revision {
                    *saved = card.clone();
                }
            } else {
                snapshot.cards.push(card.clone());
            }
            let host = snapshot.project_host(&card.project_id).to_owned();
            snapshot.entity_hosts.insert(card.id.clone(), host);
        }
        for event in self.comments.values() {
            if !snapshot.events.iter().any(|e| e.id == event.id) {
                snapshot.events.push(event.clone());
            }
        }
    }

    /// Drop a receipt only after the authoritative snapshot includes it. A
    /// newer Card revision or edited/deleted comment must always win.
    pub(super) fn reconcile(&mut self, snapshot: &mut Snapshot) {
        self.terminals
            .retain(|id, _| !snapshot.terminals.iter().any(|t| &t.id == id));
        self.cards.retain(|id, card| {
            !snapshot
                .cards
                .iter()
                .any(|c| &c.id == id && c.revision >= card.revision)
        });
        self.comments
            .retain(|id, _| !snapshot.events.iter().any(|e| &e.id == id));
        self.overlay(snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(revision: u64) -> Card {
        serde_json::from_value(json!({
            "id":"c1", "project_id":"p1", "number":1, "title":"Created",
            "body":"", "stage":"backlog", "revision":revision, "created_at":1
        }))
        .unwrap()
    }

    #[test]
    fn created_terminal_opens_from_receipt_until_snapshot_catches_up() {
        assert!(immediate("terminal.create"));
        let terminal: TerminalEntry = serde_json::from_value(json!({
            "id":"shell-new", "number":1, "name":"Terminal 1", "created_at":1,
            "runtime":{"machine":"local", "core_id":"core-1", "terminal":"pty-1",
                "state":"running", "connection":"connected"}
        }))
        .unwrap();
        let mut writes = CommittedWrites::default();
        writes.record(
            "terminal.create",
            &serde_json::to_value(&terminal).unwrap(),
            "remote",
        );
        let mut stale = Snapshot::default();
        writes.reconcile(&mut stale);
        writes.overlay(&mut stale);
        assert_eq!(stale.terminals, vec![terminal.clone()]);
        assert_eq!(stale.terminal_host(&terminal.id), "remote");
        assert_eq!(writes.terminals.len(), 1);

        let mut closed = terminal;
        closed.name = Some("Renamed elsewhere".into());
        closed.runtime.as_mut().unwrap().state = "finished".into();
        let mut fresh = Snapshot {
            terminals: vec![closed.clone()],
            ..Default::default()
        };
        writes.reconcile(&mut fresh);
        assert_eq!(fresh.terminals, vec![closed]);
        assert!(writes.terminals.is_empty());
        let mut later = Snapshot::default();
        writes.overlay(&mut later);
        assert!(later.terminals.is_empty());
    }

    #[test]
    fn receipts_survive_stale_remote_snapshots_and_yield_to_authoritative_edits() {
        let mut writes = CommittedWrites::default();
        writes.record(
            "card.create",
            &serde_json::to_value(card(2)).unwrap(),
            "local",
        );
        let event = Event {
            id: "reply-1".into(),
            project_id: "p1".into(),
            card_id: "c1".into(),
            sequence: 2,
            kind: "comment.created".into(),
            actor: "user".into(),
            message: "Sent".into(),
            data: json!({"body":"Sent"}),
            created_at: 2,
        };
        writes.record(
            "comment.add",
            &serde_json::to_value(&event).unwrap(),
            "local",
        );
        let mut stale = Snapshot::default();
        stale.entity_hosts.insert("p1".into(), "remote".into());
        writes.reconcile(&mut stale);
        writes.overlay(&mut stale);
        assert_eq!(stale.cards, vec![card(2)]);
        assert_eq!(stale.card_host("c1"), "remote");
        assert_eq!(stale.events, vec![event.clone()]);
        assert_eq!(writes.cards.len(), 1);
        assert_eq!(writes.comments.len(), 1);

        let mut newer = card(3);
        newer.title = "Updated by another client".into();
        let mut edited = event;
        edited.data = json!({"body":"Edited", "revision":2});
        let mut fresh = Snapshot {
            cards: vec![newer.clone()],
            events: vec![edited.clone()],
            ..Default::default()
        };
        writes.reconcile(&mut fresh);
        assert_eq!(fresh.cards, vec![newer]);
        assert_eq!(fresh.events, vec![edited]);
        assert!(writes.cards.is_empty());
        assert!(writes.comments.is_empty());

        // Removing a newly created Card before the first refresh must not
        // resurrect it from its creation receipt.
        writes.record(
            "card.create",
            &serde_json::to_value(card(2)).unwrap(),
            "local",
        );
        writes.record(
            "card.remove",
            &json!({"removed":true,"card_id":"c1"}),
            "local",
        );
        let mut removed = Snapshot::default();
        writes.reconcile(&mut removed);
        assert!(removed.cards.is_empty());
    }
}
