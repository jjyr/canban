//! Read-your-writes while a background (possibly cached remote) snapshot catches up.
//! Only server-confirmed replies enter this overlay; failed submissions never do.
use canban_types::{
    Card, CardRead, ConfigurationSession, Event, ManagedWorktree, ReviewDraft, ScheduleState,
    Snapshot, TerminalContext, TerminalEntry,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn config_only(method: &str) -> bool {
    matches!(
        method,
        "config.agents.save"
            | "agent.setup"
            | "scheduler.configure"
            | "host.env.refresh"
            | "utilities.configure"
            | "utilities.awake"
            | "github.notification.read"
            | "github.notification.done"
    )
}

pub(super) fn immediate(method: &str) -> bool {
    matches!(
        method,
        "card.create"
            | "card.move"
            | "card.resolve"
            | "card.reopen"
            | "card.hold_auto_merge"
            | "comment.add"
            | "comment.update"
            | "comment.delete"
            | "notifications.read"
            | "notifications.read_all"
            | "schedule.toggle"
            | "github.issue.add"
            | "github.item.add"
            | "terminal.create"
            | "review.run_agent"
            | "review.send_agent"
            | "review.draft.set"
            | "review.draft.clear"
            | "agent.open"
            | "execution.retry"
            | "configuration.open"
            | "worktree.create"
            | "worktree.promote"
    )
}

#[derive(Default)]
pub(crate) struct CommittedWrites {
    cards: BTreeMap<String, Card>,
    card_revisions: BTreeMap<String, u64>,
    comments: BTreeMap<String, Event>,
    deleted_comments: BTreeSet<String>,
    card_reads: BTreeMap<String, CardRead>,
    schedule_states: BTreeMap<String, ScheduleState>,
    terminals: BTreeMap<String, (TerminalEntry, String)>,
    terminal_hints: BTreeMap<String, TerminalContext>,
    review_drafts: BTreeMap<String, (Option<ReviewDraft>, u64)>,
    worktrees: BTreeMap<String, (ManagedWorktree, String)>,
    removed_worktrees: BTreeSet<String>,
    configuration_sessions: BTreeMap<String, ConfigurationSession>,
}

fn event_revision(event: &Event) -> u64 {
    event.data["revision"].as_u64().unwrap_or(1)
}

fn terminal_is_newer(receipt: &TerminalEntry, snap: &TerminalEntry) -> bool {
    match (&receipt.agent, &snap.agent) {
        (Some(ra), Some(sa)) => {
            ra.number > sa.number
                || (ra.number == sa.number && ra.pending.is_some() && sa.pending.is_none())
        }
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => receipt.created_at > snap.created_at,
    }
}

impl CommittedWrites {
    pub(super) fn record_terminal_hint(&mut self, hint: TerminalContext) {
        self.terminal_hints.insert(hint.terminal_id.clone(), hint);
    }

    pub(crate) fn record_card_reads(&mut self, reads: &[CardRead]) {
        for read in reads {
            self.card_reads
                .entry(read.card_id.clone())
                .and_modify(|cur| cur.sequence = cur.sequence.max(read.sequence))
                .or_insert_with(|| read.clone());
        }
    }

    fn record_card(&mut self, card: Card) {
        self.card_revisions
            .entry(card.id.clone())
            .and_modify(|cur| *cur = (*cur).max(card.revision))
            .or_insert(card.revision);
        let replace = self
            .cards
            .get(&card.id)
            .is_none_or(|prev| prev.revision <= card.revision);
        if replace {
            self.cards.insert(card.id.clone(), card);
        }
    }

    fn record_terminal(&mut self, terminal: TerminalEntry, host: &str) {
        let replace = self
            .terminals
            .get(&terminal.id)
            .is_none_or(|(prev, _)| terminal_is_newer(&terminal, prev));
        if replace {
            self.terminals
                .insert(terminal.id.clone(), (terminal, host.into()));
        }
    }

    fn record_comment(&mut self, reply: &Value) {
        if let (Some(card_id), Some(rev)) =
            (reply["card_id"].as_str(), reply["card_revision"].as_u64())
        {
            self.record_card_revision(card_id, rev);
        }
        if let Ok(event) = serde_json::from_value::<Event>(reply.clone()) {
            self.deleted_comments.remove(&event.id);
            self.comments.insert(event.id.clone(), event);
        }
    }

    pub(super) fn record(&mut self, method: &str, reply: &Value, host: &str) {
        if method == "card.remove" && reply["removed"] == true {
            if let Some(id) = reply["card_id"].as_str() {
                self.cards.remove(id);
                self.card_revisions.remove(id);
                self.comments.retain(|_, event| event.card_id != id);
                self.card_reads.remove(id);
            }
            return;
        }
        if !immediate(method) {
            return;
        }
        match method {
            "terminal.create" | "execution.retry" => {
                if let Ok(terminal) = serde_json::from_value::<TerminalEntry>(reply.clone()) {
                    self.record_terminal(terminal, host);
                }
            }
            "agent.open" => {
                if let Ok(terminal) =
                    serde_json::from_value::<TerminalEntry>(reply["terminal"].clone())
                {
                    self.record_terminal(terminal, host);
                }
            }
            "review.run_agent" => {
                if let Ok(card) = serde_json::from_value::<Card>(reply["card_record"].clone()) {
                    self.record_card(card);
                }
                if let Ok(terminal) =
                    serde_json::from_value::<TerminalEntry>(reply["terminal"].clone())
                {
                    self.record_terminal(terminal, host);
                }
            }
            "review.send_agent" => {
                if let Ok(card) = serde_json::from_value::<Card>(reply["card_record"].clone()) {
                    self.record_card(card);
                }
                if !reply["event"].is_null() {
                    self.record_comment(&reply["event"]);
                }
                if let Ok(terminal) =
                    serde_json::from_value::<TerminalEntry>(reply["terminal"].clone())
                {
                    self.record_terminal(terminal, host);
                }
                if let Ok(hint) =
                    serde_json::from_value::<TerminalContext>(reply["terminal_context"].clone())
                {
                    self.record_terminal_hint(hint);
                }
            }
            "review.draft.set" => {
                if let Ok(draft) = serde_json::from_value::<ReviewDraft>(reply["draft"].clone()) {
                    let ts = draft.updated_at;
                    self.review_drafts
                        .insert(draft.card_id.clone(), (Some(draft), ts));
                }
            }
            "review.draft.clear" => {
                if reply["cleared"] == true
                    && let Some(card_id) = reply["card_id"].as_str()
                {
                    let cleared_at = reply["cleared_at"].as_u64().unwrap_or(u64::MAX);
                    self.review_drafts
                        .insert(card_id.to_owned(), (None, cleared_at));
                }
            }
            "configuration.open" => {
                if let Ok(terminal) =
                    serde_json::from_value::<TerminalEntry>(reply["terminal"].clone())
                {
                    self.record_terminal(terminal, host);
                }
                if let Ok(session) =
                    serde_json::from_value::<ConfigurationSession>(reply["session"].clone())
                {
                    let replace = self
                        .configuration_sessions
                        .get(&session.id)
                        .is_none_or(|prev| prev.generation <= session.generation);
                    if replace {
                        self.configuration_sessions
                            .insert(session.id.clone(), session);
                    }
                }
            }
            "worktree.create" => {
                if let Ok(wt) = serde_json::from_value::<ManagedWorktree>(reply["worktree"].clone())
                {
                    self.removed_worktrees.remove(&wt.id);
                    self.worktrees.insert(wt.id.clone(), (wt, host.into()));
                }
                if let Ok(terminal) =
                    serde_json::from_value::<TerminalEntry>(reply["terminal"].clone())
                {
                    self.record_terminal(terminal, host);
                }
            }
            "worktree.promote" => {
                if let Ok(card) = serde_json::from_value::<Card>(reply.clone()) {
                    self.record_card(card);
                }
                if let Some(wt_id) = reply["promoted_from_worktree_id"].as_str() {
                    self.worktrees.remove(wt_id);
                    self.removed_worktrees.insert(wt_id.to_owned());
                }
            }
            "comment.add" | "comment.update" => {
                self.record_comment(reply);
            }
            "comment.delete" => {
                if reply["deleted"] == true {
                    if let (Some(card_id), Some(rev)) =
                        (reply["card_id"].as_str(), reply["revision"].as_u64())
                    {
                        self.record_card_revision(card_id, rev);
                    }
                    if let Some(cid) = reply["comment_id"].as_str() {
                        self.comments.remove(cid);
                        self.deleted_comments.insert(cid.to_owned());
                    }
                }
            }
            "notifications.read" => {
                if let Ok(read) = serde_json::from_value::<CardRead>(reply.clone()) {
                    self.record_card_reads(&[read]);
                }
            }
            "schedule.toggle" => {
                if let Ok(state) = serde_json::from_value::<ScheduleState>(reply.clone()) {
                    self.schedule_states.insert(state.id.clone(), state);
                }
            }
            "notifications.read_all" => {}
            _ => {
                if let Ok(card) = serde_json::from_value::<Card>(reply.clone()) {
                    self.record_card(card);
                }
            }
        }
    }

    fn record_card_revision(&mut self, card_id: &str, rev: u64) {
        self.card_revisions
            .entry(card_id.to_owned())
            .and_modify(|cur| *cur = (*cur).max(rev))
            .or_insert(rev);
        if let Some(saved) = self.cards.get_mut(card_id)
            && saved.revision < rev
        {
            saved.revision = rev;
        }
    }

    pub(super) fn overlay(&self, snapshot: &mut Snapshot) {
        for (terminal, host) in self.terminals.values() {
            // Never overwrite a newer runtime observation (exit, close or rename).
            if let Some(existing) = snapshot.terminals.iter_mut().find(|t| t.id == terminal.id) {
                if terminal_is_newer(terminal, existing) {
                    *existing = terminal.clone();
                }
            } else {
                snapshot.terminals.push(terminal.clone());
            }
            snapshot
                .entity_hosts
                .insert(terminal.id.clone(), host.clone());
        }
        for hint in self.terminal_hints.values() {
            if !hint_applies(snapshot, hint) {
                continue;
            }
            // An unknown context is not a cwd observation. Keep placement until
            // Core actually observes a directory for this exact runtime binding.
            if let Some(context) = snapshot
                .terminal_contexts
                .iter_mut()
                .find(|c| c.terminal_id == hint.terminal_id)
            {
                if context.cwd.is_none() {
                    *context = hint.clone();
                }
            } else {
                snapshot.terminal_contexts.push(hint.clone());
            }
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
        for (card_id, &rev) in &self.card_revisions {
            if let Some(saved) = snapshot.cards.iter_mut().find(|c| &c.id == card_id)
                && saved.revision < rev
            {
                saved.revision = rev;
            }
        }
        for read in self.card_reads.values() {
            if let Some(existing) = snapshot
                .card_reads
                .iter_mut()
                .find(|r| r.card_id == read.card_id)
            {
                existing.sequence = existing.sequence.max(read.sequence);
            } else {
                snapshot.card_reads.push(read.clone());
            }
        }
        for st in self.schedule_states.values() {
            if let Some(existing) = snapshot.schedule_states.iter_mut().find(|s| s.id == st.id) {
                if st.updated_at >= existing.updated_at {
                    *existing = st.clone();
                }
            } else {
                snapshot.schedule_states.push(st.clone());
            }
        }
        for (card_id, (draft_opt, ts)) in &self.review_drafts {
            match draft_opt {
                Some(draft) => {
                    if let Some(existing) = snapshot
                        .review_drafts
                        .iter_mut()
                        .find(|d| d.card_id == draft.card_id)
                    {
                        if existing.updated_at < draft.updated_at {
                            *existing = draft.clone();
                        }
                    } else {
                        snapshot.review_drafts.push(draft.clone());
                    }
                }
                None => {
                    snapshot
                        .review_drafts
                        .retain(|d| !(d.card_id == *card_id && d.updated_at <= *ts));
                }
            }
        }
        if !self.removed_worktrees.is_empty() {
            snapshot
                .worktrees
                .retain(|w| !self.removed_worktrees.contains(&w.id));
        }
        for (wt, host) in self.worktrees.values() {
            if !snapshot.worktrees.iter().any(|w| w.id == wt.id) {
                snapshot.worktrees.push(wt.clone());
            }
            snapshot.entity_hosts.insert(wt.id.clone(), host.clone());
        }
        for session in self.configuration_sessions.values() {
            if let Some(existing) = snapshot
                .configuration_sessions
                .iter_mut()
                .find(|s| s.id == session.id)
            {
                if existing.generation < session.generation {
                    *existing = session.clone();
                }
            } else {
                snapshot.configuration_sessions.push(session.clone());
            }
        }
    }

    /// Drop a receipt only after the authoritative snapshot includes it. A
    /// newer Card revision or edited/deleted comment must always win.
    pub(super) fn reconcile(&mut self, snapshot: &mut Snapshot) {
        self.terminal_hints.retain(|id, hint| {
            (hint_applies(snapshot, hint) || self.terminals.contains_key(id))
                && !snapshot
                    .terminal_contexts
                    .iter()
                    .any(|c| c.terminal_id == *id && c.cwd.is_some())
        });
        self.terminals.retain(|id, (terminal, _)| {
            match snapshot.terminals.iter().find(|t| &t.id == id) {
                None => true,
                Some(snap) => terminal_is_newer(terminal, snap),
            }
        });
        self.cards.retain(|id, card| {
            !snapshot
                .cards
                .iter()
                .any(|c| &c.id == id && c.revision >= card.revision)
        });
        self.card_revisions.retain(|id, rev| {
            snapshot
                .cards
                .iter()
                .any(|c| &c.id == id && c.revision < *rev)
                || self.cards.contains_key(id)
        });
        self.card_reads.retain(|id, read| {
            !snapshot
                .card_reads
                .iter()
                .any(|r| &r.card_id == id && r.sequence >= read.sequence)
        });
        self.schedule_states.retain(|id, st| {
            !snapshot
                .schedule_states
                .iter()
                .any(|s| &s.id == id && s.updated_at >= st.updated_at)
        });
        self.review_drafts
            .retain(|card_id, (draft_opt, ts)| match draft_opt {
                Some(draft) => !snapshot
                    .review_drafts
                    .iter()
                    .any(|d| d.card_id == *card_id && d.updated_at >= draft.updated_at),
                None => snapshot
                    .review_drafts
                    .iter()
                    .any(|d| d.card_id == *card_id && d.updated_at <= *ts),
            });
        self.worktrees
            .retain(|id, _| !snapshot.worktrees.iter().any(|w| &w.id == id));
        self.removed_worktrees
            .retain(|id| snapshot.worktrees.iter().any(|w| &w.id == id));
        self.configuration_sessions.retain(|id, session| {
            !snapshot
                .configuration_sessions
                .iter()
                .any(|s| &s.id == id && s.generation >= session.generation)
        });
        self.overlay(snapshot);
    }

    pub(super) fn overlay_card_events(&self, card_id: &str, events: &mut Vec<Event>) {
        let mut inserted = false;
        for event in self.comments.values().filter(|e| e.card_id == card_id) {
            if let Some(saved) = events.iter_mut().find(|e| e.id == event.id) {
                if saved.kind != "comment.deleted" && event_revision(event) > event_revision(saved)
                {
                    *saved = event.clone();
                }
            } else {
                events.push(event.clone());
                inserted = true;
            }
        }
        if !self.deleted_comments.is_empty() {
            for ev in events.iter_mut() {
                if self.deleted_comments.contains(&ev.id) && ev.kind != "comment.deleted" {
                    ev.kind = "comment.deleted".into();
                    ev.message = "Comment deleted".into();
                    ev.data["comment_id"] = json!(ev.id);
                }
            }
        }
        if inserted {
            events.sort_by_key(|e| e.sequence);
        }
    }

    pub(super) fn reconcile_card_events(&mut self, card_id: &str, authoritative: &[Event]) {
        self.comments.retain(|id, ev| {
            if ev.card_id != card_id {
                return true;
            }
            !authoritative.iter().any(|e| {
                let same_target = e.id == *id || e.data["comment_id"].as_str() == Some(id.as_str());
                same_target
                    && (event_revision(e) >= event_revision(ev) || e.kind == "comment.deleted")
            })
        });
        self.deleted_comments.retain(|id| {
            !authoritative.iter().any(|e| {
                e.kind == "comment.deleted"
                    && (e.id == *id || e.data["comment_id"].as_str() == Some(id.as_str()))
            })
        });
    }
}

fn hint_applies(snapshot: &Snapshot, hint: &TerminalContext) -> bool {
    snapshot.terminals.iter().any(|t| {
        t.id == hint.terminal_id
            && t.runtime.as_ref().is_some_and(|r| {
                r.state != "finished"
                    && r.machine == hint.machine
                    && r.core_id.as_deref() == Some(hint.core_id.as_str())
                    && r.terminal.as_deref() == Some(hint.terminal.as_str())
            })
    })
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
    fn creation_placement_survives_unknown_context_then_yields_to_observation() {
        let terminal: TerminalEntry = serde_json::from_value(json!({
            "id":"new", "number":1, "created_at":1,
            "runtime":{"machine":"local", "core_id":"core", "terminal":"pty",
                "state":"running", "connection":"connected"}
        }))
        .unwrap();
        let hint = TerminalContext {
            terminal_id: "new".into(),
            machine: "local".into(),
            core_id: "core".into(),
            terminal: "pty".into(),
            cwd: Some("/known".into()),
            directory_root: Some("/known".into()),
            ..Default::default()
        };
        let mut writes = CommittedWrites::default();
        writes.record(
            "terminal.create",
            &serde_json::to_value(&terminal).unwrap(),
            "local",
        );
        writes.record_terminal_hint(hint.clone());
        let mut stale = Snapshot::default();
        writes.reconcile(&mut stale);
        assert_eq!(stale.terminal_contexts, vec![hint.clone()]);
        let unknown = TerminalContext {
            cwd: None,
            ..hint.clone()
        };
        let fresh = Snapshot {
            terminals: vec![terminal.clone()],
            terminal_contexts: vec![unknown],
            ..Default::default()
        };
        let mut applied = fresh.clone();
        writes.reconcile(&mut applied);
        assert_eq!(applied.terminal_contexts, vec![hint.clone()]);
        // The entry itself arriving does not discard its placement hint.
        assert!(writes.terminals.is_empty());
        assert_eq!(writes.terminal_hints.len(), 1);
        for state in ["observed", "finished", "replaced", "removed"] {
            let mut writes = CommittedWrites::default();
            writes.record_terminal_hint(hint.clone());
            let mut s = fresh.clone();
            match state {
                "observed" => {
                    s.terminal_contexts[0].cwd = Some("/actually-elsewhere".into());
                    s.terminal_contexts[0].directory_root = Some("/actually-elsewhere".into());
                }
                "finished" => s.terminals[0].runtime.as_mut().unwrap().state = "finished".into(),
                "replaced" => {
                    s.terminals[0].runtime.as_mut().unwrap().terminal = Some("other-pty".into())
                }
                "removed" => s.terminals.clear(),
                _ => unreachable!(),
            }
            let expected = s.terminal_contexts.clone();
            writes.reconcile(&mut s);
            assert!(writes.terminal_hints.is_empty(), "{state}");
            assert_eq!(s.terminal_contexts, expected, "{state}");
        }
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
        let mut stale_events = Vec::new();
        writes.reconcile(&mut stale);
        writes.overlay(&mut stale);
        writes.reconcile_card_events("c1", &stale_events);
        writes.overlay_card_events("c1", &mut stale_events);
        assert_eq!(stale.cards, vec![card(2)]);
        assert_eq!(stale.card_host("c1"), "remote");
        assert_eq!(stale_events, vec![event.clone()]);
        assert_eq!(writes.cards.len(), 1);
        assert_eq!(writes.comments.len(), 1);

        let mut newer = card(3);
        newer.title = "Updated by another client".into();
        let mut edited = event;
        edited.data = json!({"body":"Edited", "revision":2});
        let mut fresh = Snapshot {
            cards: vec![newer.clone()],
            ..Default::default()
        };
        let mut fresh_events = vec![edited.clone()];
        writes.reconcile(&mut fresh);
        writes.reconcile_card_events("c1", &fresh_events);
        writes.overlay_card_events("c1", &mut fresh_events);
        assert_eq!(fresh.cards, vec![newer]);
        assert_eq!(fresh_events, vec![edited]);
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

    #[test]
    fn immediate_mutations_overlay_and_reconcile_across_cards_comments_reads_and_schedules() {
        assert!(config_only("config.agents.save"));
        assert!(config_only("utilities.configure"));
        assert!(config_only("utilities.awake"));
        assert!(config_only("scheduler.configure"));
        assert!(config_only("host.env.refresh"));
        assert!(immediate("card.move"));
        assert!(immediate("card.hold_auto_merge"));
        assert!(immediate("comment.update"));
        assert!(immediate("comment.delete"));
        assert!(immediate("notifications.read"));
        assert!(immediate("schedule.toggle"));

        let mut writes = CommittedWrites::default();
        let mut moved = card(2);
        moved.stage = "develop".into();
        moved.hold_auto_merge = true;
        writes.record(
            "card.move",
            &serde_json::to_value(&moved).unwrap(),
            "remote",
        );

        let updated_event = Event {
            id: "ev1".into(),
            project_id: "p1".into(),
            card_id: "c1".into(),
            sequence: 2,
            kind: "comment.created".into(),
            actor: "user".into(),
            message: "Edited".into(),
            data: json!({"body":"Edited","revision":2}),
            created_at: 2,
        };
        writes.record(
            "comment.update",
            &serde_json::to_value(&updated_event).unwrap(),
            "remote",
        );

        writes.record(
            "notifications.read",
            &json!({"id":"c1","project_id":"p1","card_id":"c1","sequence":2}),
            "local",
        );
        writes.record(
            "schedule.toggle",
            &json!({
                "id":"sched1",
                "project_id":"p1",
                "filename":"nightly.yaml",
                "workflow_id":"nightly",
                "workflow_name":"Nightly",
                "rule_id":"nightly",
                "cron":"0 0 * * *",
                "creates_card":true,
                "enabled":false,
                "updated_at":200
            }),
            "local",
        );

        let mut stale = Snapshot {
            cards: vec![card(1)],
            card_reads: vec![CardRead {
                id: "c1".into(),
                project_id: "p1".into(),
                card_id: "c1".into(),
                sequence: 1,
            }],
            schedule_states: vec![
                serde_json::from_value(json!({
                    "id":"sched1",
                    "project_id":"p1",
                    "filename":"nightly.yaml",
                    "workflow_id":"nightly",
                    "workflow_name":"Nightly",
                    "rule_id":"nightly",
                    "cron":"0 0 * * *",
                    "creates_card":true,
                    "enabled":true,
                    "next_run_at":999,
                    "updated_at":100
                }))
                .unwrap(),
            ],
            ..Default::default()
        };
        let mut stale_events = vec![Event {
            id: "ev1".into(),
            project_id: "p1".into(),
            card_id: "c1".into(),
            sequence: 2,
            kind: "comment.created".into(),
            actor: "user".into(),
            message: "Original".into(),
            data: json!({"body":"Original","revision":1}),
            created_at: 2,
        }];
        writes.reconcile(&mut stale);
        writes.reconcile_card_events("c1", &stale_events);
        writes.overlay_card_events("c1", &mut stale_events);
        assert_eq!(stale.cards[0].stage, "develop");
        assert!(stale.cards[0].hold_auto_merge);
        assert_eq!(stale_events[0].message, "Edited");
        assert_eq!(stale.card_reads[0].sequence, 2);
        assert!(!stale.schedule_states[0].enabled);

        writes.record(
            "comment.delete",
            &json!({"deleted":true,"comment_id":"ev1","card_id":"c1","revision":3}),
            "local",
        );
        writes.overlay(&mut stale);
        writes.overlay_card_events("c1", &mut stale_events);
        assert_eq!(stale_events[0].kind, "comment.deleted");
        assert_eq!(stale.cards[0].revision, 3);

        let mut fresh = stale.clone();
        fresh.schedule_states[0].updated_at = 200;
        let fresh_events = stale_events.clone();
        writes.reconcile(&mut fresh);
        writes.reconcile_card_events("c1", &fresh_events);
        assert!(writes.cards.is_empty());
        assert!(writes.card_revisions.is_empty());
        assert!(writes.comments.is_empty());
        assert!(writes.deleted_comments.is_empty());
        assert!(writes.card_reads.is_empty());
        assert!(writes.schedule_states.is_empty());

        // A comment receipt carrying a newer `card_revision` advances a stale snapshot's Card revision.
        let mut stale_comment = Snapshot {
            cards: vec![card(1)],
            ..Default::default()
        };
        let mut comment_reply = serde_json::to_value(Event {
            id: "ev2".into(),
            project_id: "p1".into(),
            card_id: "c1".into(),
            sequence: 3,
            kind: "comment.created".into(),
            actor: "user".into(),
            message: "Bumped".into(),
            data: json!({"body":"Bumped"}),
            created_at: 3,
        })
        .unwrap();
        comment_reply["card_revision"] = json!(4);
        writes.record("comment.add", &comment_reply, "local");
        writes.reconcile(&mut stale_comment);
        assert_eq!(stale_comment.cards[0].revision, 4);
    }

    #[test]
    fn review_agent_draft_worktree_and_configuration_receipts_overlay_and_reconcile() {
        for method in [
            "review.run_agent",
            "review.send_agent",
            "review.draft.set",
            "review.draft.clear",
            "agent.open",
            "execution.retry",
            "configuration.open",
            "worktree.create",
            "worktree.promote",
        ] {
            assert!(immediate(method), "{method} should be immediate");
        }

        let mut writes = CommittedWrites::default();

        // 1. review.run_agent updates an existing TerminalEntry when agent.number increases.
        let stale_term: TerminalEntry = serde_json::from_value(json!({
            "id":"conv:c1:reviewer", "number":1, "created_at":1,
            "agent":{
                "project_id":"p1", "card_id":"c1", "card_number":1, "number":1,
                "stage":"review", "trigger":"diff_review",
                "profile":{"id":"reviewer","name":"Reviewer","command":["echo"]},
                "instruction":"", "state":"completed"
            }
        }))
        .unwrap();
        let mut next_term = stale_term.clone();
        next_term.agent.as_mut().unwrap().number = 2;
        next_term.agent.as_mut().unwrap().state = "queued".into();
        writes.record(
            "review.run_agent",
            &json!({
                "state":"queued",
                "card_id":"c1",
                "card_record": card(2),
                "terminal": next_term,
            }),
            "local",
        );

        // 2. review.send_agent (terminal) overlays terminal + terminal_context.
        let review_term: TerminalEntry = serde_json::from_value(json!({
            "id":"review-agent-1", "number":2, "origin_project_id":"p1", "created_at":10,
            "runtime":{"machine":"local", "core_id":"core-1", "terminal":"pty-9",
                "state":"running", "connection":"connected"}
        }))
        .unwrap();
        let review_ctx = TerminalContext {
            terminal_id: "review-agent-1".into(),
            machine: "local".into(),
            core_id: "core-1".into(),
            terminal: "pty-9".into(),
            cwd: Some("/repo".into()),
            directory_root: Some("/repo".into()),
            project_id: Some("p1".into()),
            ..Default::default()
        };
        writes.record(
            "review.send_agent",
            &json!({
                "state":"running",
                "mode":"terminal",
                "terminal_id":"review-agent-1",
                "terminal": review_term,
                "terminal_context": review_ctx,
            }),
            "local",
        );

        // 3. review.draft.set overlays draft, then review.draft.clear hides it.
        writes.record(
            "review.draft.set",
            &json!({
                "saved": true,
                "draft": {
                    "card_id":"c1",
                    "summary":"Check lock",
                    "verdict":"comment",
                    "comments":[],
                    "updated_at": 100
                }
            }),
            "local",
        );
        let mut stale = Snapshot {
            cards: vec![card(1)],
            terminals: vec![stale_term],
            ..Default::default()
        };
        writes.reconcile(&mut stale);
        assert_eq!(stale.cards[0].revision, 2);
        assert_eq!(
            stale
                .terminals
                .iter()
                .find(|t| t.id == "conv:c1:reviewer")
                .unwrap()
                .agent
                .as_ref()
                .unwrap()
                .state,
            "queued"
        );
        assert!(stale.terminals.iter().any(|t| t.id == "review-agent-1"));
        assert!(stale
            .terminal_contexts
            .iter()
            .any(|c| c.terminal_id == "review-agent-1" && c.project_id.as_deref() == Some("p1")));
        assert_eq!(stale.review_drafts.len(), 1);
        assert_eq!(stale.review_drafts[0].summary, "Check lock");

        writes.record(
            "review.draft.clear",
            &json!({"cleared": true, "card_id": "c1", "cleared_at": 150}),
            "local",
        );
        writes.overlay(&mut stale);
        assert!(stale.review_drafts.is_empty());

        // Reconcile drops cleared marker once snapshot no longer has the draft.
        let mut fresh = stale.clone();
        writes.reconcile(&mut fresh);
        assert!(writes.review_drafts.is_empty());
        assert!(writes.cards.is_empty());
        assert!(writes.terminals.is_empty());

        // 4. worktree.create adds ManagedWorktree, then worktree.promote replaces it with Card.
        writes.record(
            "worktree.create",
            &json!({
                "worktree": {
                    "id": "wt1",
                    "project_id": "p1",
                    "branch": "feat/wt",
                    "base_branch": "main",
                    "path": "/tmp/wt1",
                    "location": {"machine": "local", "directory": "/repo"},
                    "created_at": 10
                },
                "terminal": null
            }),
            "local",
        );
        let mut wt_snap = Snapshot::default();
        writes.reconcile(&mut wt_snap);
        assert_eq!(wt_snap.worktrees.len(), 1);
        assert_eq!(wt_snap.worktrees[0].id, "wt1");

        let mut promoted = card(1);
        promoted.id = "c2".into();
        let mut promoted_reply = serde_json::to_value(&promoted).unwrap();
        promoted_reply["promoted_from_worktree_id"] = json!("wt1");
        writes.record("worktree.promote", &promoted_reply, "local");
        writes.overlay(&mut wt_snap);
        assert!(wt_snap.worktrees.is_empty());
        assert!(wt_snap.cards.iter().any(|c| c.id == "c2"));

        // 5. configuration.open overlays ConfigurationSession and TerminalEntry.
        writes.record(
            "configuration.open",
            &json!({
                "terminal_id": "configuration:p1:workflows:developer",
                "reused": false,
                "state": "queued",
                "terminal": {
                    "id": "configuration:p1:workflows:developer",
                    "number": 5,
                    "origin_project_id": "p1",
                    "created_at": 50
                },
                "session": {
                    "id": "configuration:p1:workflows:developer",
                    "project_id": "p1",
                    "profile_id": "developer",
                    "provider_session_id": null,
                    "generation": 1,
                    "state": "queued",
                    "prompt": "p",
                    "error": null
                }
            }),
            "local",
        );
        let mut cfg_snap = Snapshot::default();
        writes.reconcile(&mut cfg_snap);
        assert_eq!(cfg_snap.configuration_sessions.len(), 1);
        assert_eq!(cfg_snap.terminals.len(), 1);
    }

    #[test]
    fn resolve_and_reopen_overlay_immediately_and_reconcile() {
        assert!(immediate("card.resolve"));
        assert!(immediate("card.reopen"));

        let mut writes = CommittedWrites::default();
        let initial = card(1);
        let mut resolved = card(2);
        resolved.resolution = Some(canban_types::CardResolution {
            resolved_at: 123,
            kind: "manual".into(),
            reason: None,
            actor: "user".into(),
        });

        writes.record(
            "card.resolve",
            &serde_json::to_value(&resolved).unwrap(),
            "local",
        );
        let mut stale = Snapshot {
            cards: vec![initial.clone()],
            ..Snapshot::default()
        };
        writes.overlay(&mut stale);
        assert!(stale.cards[0].resolved());
        assert_eq!(stale.cards[0].revision, 2);

        let mut fresh_resolved = Snapshot {
            cards: vec![resolved.clone()],
            ..Snapshot::default()
        };
        writes.reconcile(&mut fresh_resolved);
        assert!(writes.cards.is_empty());

        let reopened = card(3);
        writes.record(
            "card.reopen",
            &serde_json::to_value(&reopened).unwrap(),
            "local",
        );
        let mut stale_resolved = Snapshot {
            cards: vec![resolved],
            ..Snapshot::default()
        };
        writes.overlay(&mut stale_resolved);
        assert!(!stale_resolved.cards[0].resolved());
        assert_eq!(stale_resolved.cards[0].revision, 3);
    }
}
