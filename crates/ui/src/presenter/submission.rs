//! Read-your-writes while a background (possibly cached remote) snapshot catches up.
//! Only server-confirmed replies enter this overlay; failed submissions never do.
use canban_types::{
    Card, CardRead, ConfigurationSession, EnvEntryMeta, Event, ManagedWorktree, Project,
    ReviewDraft, ScheduleState, Snapshot, TerminalContext, TerminalEntry,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn config_only(method: &str) -> bool {
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
            | "update.check"
            | "update.install"
    )
}

pub fn immediate(method: &str) -> bool {
    matches!(
        method,
        "card.create"
            | "card.update"
            | "card.move"
            | "card.archive"
            | "card.restore"
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
            | "env.save"
            | "project.agents"
    )
}

#[derive(Default, Debug)]
pub struct CommittedWrites {
    cards: BTreeMap<String, Card>,
    card_revisions: BTreeMap<String, u64>,
    comments: BTreeMap<String, Event>,
    deleted_comments: BTreeSet<String>,
    card_reads: BTreeMap<String, CardRead>,
    schedule_states: BTreeMap<String, ScheduleState>,
    terminals: BTreeMap<String, (TerminalEntry, String)>,
    terminal_hints: BTreeMap<String, TerminalContext>,
    last_terminal_contexts: BTreeMap<String, TerminalContext>,
    review_drafts: BTreeMap<String, (Option<ReviewDraft>, u64)>,
    worktrees: BTreeMap<String, (ManagedWorktree, String)>,
    removed_worktrees: BTreeSet<String>,
    configuration_sessions: BTreeMap<String, ConfigurationSession>,
    projects: BTreeMap<String, Project>,
    env_updates: BTreeMap<String, (BTreeMap<String, EnvEntryMeta>, u64)>,
    env_gen: u64,
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

fn live_terminal_binding(t: &TerminalEntry) -> Option<(&str, &str)> {
    if let Some(a) = &t.agent {
        if a.finished() || a.connection_state == "replaced" {
            return None;
        }
        return Some((a.core_id.as_deref()?, a.terminal.as_deref()?));
    }
    let r = t.runtime.as_ref()?;
    if r.state == "finished" || r.connection == "replaced" {
        return None;
    }
    Some((r.core_id.as_deref()?, r.terminal.as_deref()?))
}

impl CommittedWrites {
    pub fn record_terminal_hint(&mut self, hint: TerminalContext) {
        self.terminal_hints.insert(hint.terminal_id.clone(), hint);
    }

    pub fn pending_card_count(&self) -> usize {
        self.cards.len()
    }

    pub fn pending_card_ids(&self) -> Vec<String> {
        self.cards.keys().cloned().collect()
    }

    pub fn pending_card_revisions(&self) -> &BTreeMap<String, u64> {
        &self.card_revisions
    }

    pub fn pending_comment_count(&self) -> usize {
        self.comments.len()
    }

    pub fn pending_comment_card_ids(&self) -> BTreeSet<String> {
        self.comments.values().map(|e| e.card_id.clone()).collect()
    }

    pub fn pending_deleted_comment_count(&self) -> usize {
        self.deleted_comments.len()
    }

    pub fn pending_terminal_count(&self) -> usize {
        self.terminals.len()
    }

    pub fn pending_terminal_hint_count(&self) -> usize {
        self.terminal_hints.len()
    }

    pub fn pending_card_read_count(&self) -> usize {
        self.card_reads.len()
    }

    pub fn pending_schedule_state_count(&self) -> usize {
        self.schedule_states.len()
    }

    pub fn pending_review_draft_count(&self) -> usize {
        self.review_drafts.len()
    }

    pub fn pending_worktree_count(&self) -> usize {
        self.worktrees.len() + self.removed_worktrees.len()
    }

    pub fn pending_configuration_session_count(&self) -> usize {
        self.configuration_sessions.len()
    }

    pub fn pending_project_count(&self) -> usize {
        self.projects.len()
    }

    pub fn pending_env_update_count(&self) -> usize {
        self.env_updates.len()
    }

    pub fn last_terminal_context_count(&self) -> usize {
        self.last_terminal_contexts.len()
    }

    fn record_observed_terminal_contexts(&mut self, snapshot: &Snapshot) {
        for ctx in &snapshot.terminal_contexts {
            if ctx.cwd.is_none() {
                continue;
            }
            if snapshot.terminals.iter().any(|t| {
                t.id == ctx.terminal_id
                    && live_terminal_binding(t)
                        == Some((ctx.core_id.as_str(), ctx.terminal.as_str()))
            }) {
                self.last_terminal_contexts
                    .insert(ctx.terminal_id.clone(), ctx.clone());
            }
        }
        self.last_terminal_contexts.retain(|id, ctx| {
            ctx.project_id
                .as_deref()
                .is_none_or(|pid| snapshot.projects.iter().any(|p| p.id == pid))
                && snapshot.terminals.iter().any(|t| {
                    t.id == *id
                        && live_terminal_binding(t)
                            == Some((ctx.core_id.as_str(), ctx.terminal.as_str()))
                })
        });
    }

    fn apply_last_terminal_contexts(&self, snapshot: &mut Snapshot) {
        for (tid, cached) in &self.last_terminal_contexts {
            let still_live_same_binding = snapshot.terminals.iter().any(|t| {
                t.id == *tid
                    && live_terminal_binding(t)
                        == Some((cached.core_id.as_str(), cached.terminal.as_str()))
            });
            if !still_live_same_binding {
                continue;
            }
            if let Some(existing) = snapshot
                .terminal_contexts
                .iter_mut()
                .find(|c| c.terminal_id == *tid)
            {
                if existing.cwd.is_none() {
                    *existing = cached.clone();
                }
            } else {
                snapshot.terminal_contexts.push(cached.clone());
            }
        }
    }

    pub fn record_card_reads(&mut self, reads: &[CardRead]) {
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
        let mut normalized = reply.clone();
        if let Some(obj) = normalized.as_object_mut()
            && obj.contains_key("message")
            && obj.contains_key("body")
        {
            obj.remove("body");
        }
        if let Ok(event) = serde_json::from_value::<Event>(normalized) {
            self.deleted_comments.remove(&event.id);
            self.comments.insert(event.id.clone(), event);
        }
    }

    pub fn record(&mut self, method: &str, reply: &Value, host: &str) {
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
                let created_wt =
                    serde_json::from_value::<ManagedWorktree>(reply["worktree"].clone()).ok();
                if let Some(wt) = created_wt.clone() {
                    self.removed_worktrees.remove(&wt.id);
                    self.worktrees.insert(wt.id.clone(), (wt, host.into()));
                }
                if let Ok(terminal) =
                    serde_json::from_value::<TerminalEntry>(reply["terminal"].clone())
                {
                    if let (Some(wt), Some(runtime)) =
                        (created_wt.as_ref(), terminal.runtime.as_ref())
                        && let (Some(core_id), Some(pty_id)) =
                            (runtime.core_id.as_ref(), runtime.terminal.as_ref())
                    {
                        self.record_terminal_hint(TerminalContext {
                            terminal_id: terminal.id.clone(),
                            core_id: core_id.clone(),
                            terminal: pty_id.clone(),
                            cwd: Some(wt.path.clone()),
                            directory_root: Some(wt.path.clone()),
                            project_id: Some(wt.project_id.clone()),
                            card_id: None,
                            worktree_id: Some(wt.id.clone()),
                            branch: Some(wt.branch.clone()),
                            dirty: false,
                            ..Default::default()
                        });
                    }
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
                if let Ok(mut state) = serde_json::from_value::<ScheduleState>(reply.clone()) {
                    if state.host_id.is_empty() {
                        state.host_id = host.to_owned();
                    }
                    self.schedule_states.insert(state.id.clone(), state);
                }
            }
            "env.save" => {
                if let Ok(meta) =
                    serde_json::from_value::<BTreeMap<String, EnvEntryMeta>>(reply["env"].clone())
                {
                    let scope = reply["scope"].as_str().unwrap_or("");
                    let target_id = reply["target_id"].as_str().unwrap_or("");
                    let snap_key = reply["snapshot_key"].as_str().unwrap_or("");
                    let host_id = host.trim();
                    let key = if scope == "global" {
                        "global".to_owned()
                    } else if scope == "agent" && !host_id.is_empty() && host_id != "local" {
                        Snapshot::agent_env_key(host_id, target_id)
                    } else {
                        snap_key.to_owned()
                    };
                    if !key.is_empty() {
                        self.env_gen = self.env_gen.saturating_add(1);
                        self.env_updates.insert(key, (meta, self.env_gen));
                    }
                }
            }
            "project.agents" => {
                if let Ok(project) = serde_json::from_value::<Project>(reply.clone()) {
                    self.projects.insert(project.id.clone(), project);
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

    pub fn overlay(&self, snapshot: &mut Snapshot) {
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
        for project in self.projects.values() {
            if let Some(existing) = snapshot.projects.iter_mut().find(|p| p.id == project.id) {
                *existing = project.clone();
            }
        }
        for (key, (meta, _)) in &self.env_updates {
            if key == "global" {
                snapshot.global_env = meta.clone();
            } else if meta.is_empty() {
                snapshot.scoped_env.remove(key);
            } else {
                snapshot.scoped_env.insert(key.clone(), meta.clone());
            }
        }
    }

    /// Reconcile only terminal and terminal-context receipts for a lightweight
    /// `runtime.state` refresh, leaving durable card/draft/project receipts intact.
    pub fn reconcile_runtime(&mut self, snapshot: &mut Snapshot) {
        self.record_observed_terminal_contexts(snapshot);
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
        for (terminal, host) in self.terminals.values() {
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
        self.apply_last_terminal_contexts(snapshot);
    }

    /// Drop a receipt only after the authoritative snapshot includes it. A
    /// newer Card revision or edited/deleted comment must always win.
    pub fn reconcile(&mut self, snapshot: &mut Snapshot) {
        self.record_observed_terminal_contexts(snapshot);
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
        self.projects
            .retain(|id, proj| snapshot.projects.iter().any(|p| &p.id == id && p != proj));
        self.env_updates.retain(|key, (meta, _)| {
            if key == "global" {
                snapshot.global_env != *meta
            } else if meta.is_empty() {
                snapshot.scoped_env.get(key).is_some_and(|m| !m.is_empty())
            } else {
                snapshot.scoped_env.get(key) != Some(meta)
            }
        });
        self.overlay(snapshot);
        self.apply_last_terminal_contexts(snapshot);
    }

    pub fn overlay_card_events(&self, card_id: &str, events: &mut Vec<Event>) {
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

    pub fn reconcile_card_events(&mut self, card_id: &str, authoritative: &[Event]) {
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
                    && r.core_id.as_deref() == Some(hint.core_id.as_str())
                    && r.terminal.as_deref() == Some(hint.terminal.as_str())
            })
    })
}
