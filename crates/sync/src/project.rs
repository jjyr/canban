//! Optimistic snapshot projection, synthetic offline receipts, and offline resolve previews.
use crate::outbox::{Mutation, MutationKind, MutationState};
use anyhow::{Context, Result};
use canban_types::{
    Card, CardContent, CardResolution, Event, Snapshot, SyncMark, agent_activity_warning,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn next_backlog_board_order(snapshot: &Snapshot, project_id: &str) -> i64 {
    snapshot
        .cards
        .iter()
        .filter(|c| {
            c.active() && !c.resolved() && c.project_id == project_id && c.stage == "backlog"
        })
        .map(Card::board_rank)
        .max()
        .map_or(0, |max| max.saturating_add(1024))
}

fn build_offline_card(op: &Mutation, snapshot: &Snapshot) -> Result<Card> {
    let project_id = op.params["project_id"]
        .as_str()
        .context("Missing project_id in card.create")?
        .to_owned();
    let title = op.params["title"]
        .as_str()
        .context("Missing title in card.create")?
        .trim()
        .to_owned();
    let body = op.params["body"].as_str().unwrap_or("").to_owned();
    let order = next_backlog_board_order(snapshot, &project_id);
    Ok(Card {
        schedule_source: None,
        source_trust: Some("local".into()),
        hold_auto_merge: false,
        github_links: Vec::new(),
        created_by: "Local".into(),
        content: CardContent::Local,
        removed_at: None,
        pull_request: None,
        pull_request_history: Vec::new(),
        delivery_mode: None,
        workspace: None,
        workspace_history: Vec::new(),
        id: op.card_id.clone(),
        project_id,
        number: 0,
        board_order: Some(order),
        title,
        body,
        context_notes: None,
        stage: "backlog".into(),
        revision: 0,
        attention: None,
        archived_at: None,
        archive: None,
        archive_error: None,
        worktree: None,
        branch: None,
        base_commit: None,
        human_gate: None,
        delivery: None,
        resolution: None,
        created_at: op.created_at,
    })
}

fn apply_one(snapshot: &mut Snapshot, host_id: &str, op: &Mutation) {
    match op.kind {
        MutationKind::CreateCard => {
            if !snapshot.cards.iter().any(|c| c.id == op.card_id)
                && let Ok(card) = build_offline_card(op, snapshot)
            {
                snapshot
                    .entity_hosts
                    .insert(card.id.clone(), host_id.to_owned());
                snapshot.cards.push(card);
            }
        }
        MutationKind::UpdateCard => {
            if let Some(card) = snapshot.cards.iter_mut().find(|c| c.id == op.card_id) {
                if let Some(t) = op.params.get("title").and_then(Value::as_str) {
                    let trimmed = t.trim();
                    if !trimmed.is_empty() {
                        card.title = trimmed.to_owned();
                    }
                }
                if let Some(b) = op.params.get("body").and_then(Value::as_str) {
                    card.body = b.to_owned();
                }
            }
        }
        MutationKind::MoveCard => {
            if let Some(stage) = op.params.get("stage").and_then(Value::as_str)
                && let Some(card) = snapshot.cards.iter_mut().find(|c| c.id == op.card_id)
            {
                card.stage = stage.to_owned();
                card.attention = None;
                card.human_gate = None;
            }
        }
        MutationKind::ResolveCard => {
            if let Some(card) = snapshot.cards.iter_mut().find(|c| c.id == op.card_id) {
                let reason = op.params["message"]
                    .as_str()
                    .or_else(|| op.params["reason"].as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                card.resolution = Some(CardResolution {
                    resolved_at: op.created_at,
                    kind: "manual".into(),
                    reason,
                    actor: "user".into(),
                });
                card.attention = None;
                card.human_gate = None;
            }
        }
        MutationKind::AddComment => {}
    }
}

/// Compute per-card sync markers from an ordered slice of pending mutations.
pub fn compute_pending_sync(ops: &[Mutation]) -> BTreeMap<String, SyncMark> {
    let mut marks = BTreeMap::new();
    for op in ops {
        let next = match op.state {
            MutationState::Failed { .. } => SyncMark::Failed,
            MutationState::Conflict { .. } => SyncMark::Conflict,
            MutationState::Queued | MutationState::InFlight => SyncMark::Pending,
        };
        marks
            .entry(op.card_id.clone())
            .and_modify(|cur| {
                // Failed > Conflict > Pending
                let rank = |m: SyncMark| match m {
                    SyncMark::Pending => 1,
                    SyncMark::Conflict => 2,
                    SyncMark::Failed => 3,
                };
                if rank(next) > rank(*cur) {
                    *cur = next;
                }
            })
            .or_insert(next);
    }
    marks
}

/// Apply all queued mutations onto a clone/working `Snapshot` and populate `pending_sync`.
pub fn apply(snapshot: &mut Snapshot, host_id: &str, ops: &[Mutation]) {
    for op in ops {
        apply_one(snapshot, host_id, op);
    }
    for (card_id, mark) in compute_pending_sync(ops) {
        snapshot.pending_sync.insert(card_id, mark);
    }
}

/// Build the synthetic RPC receipt returned to callers when `op` is queued offline.
pub fn synthetic_receipt(
    op: &Mutation,
    base_snapshot: &Snapshot,
    all_ops: &[Mutation],
    host_id: &str,
) -> Result<Value> {
    let mut snap = base_snapshot.clone();
    apply(&mut snap, host_id, all_ops);
    if !all_ops.iter().any(|m| m.id == op.id) {
        apply_one(&mut snap, host_id, op);
    }
    let card_opt = snap.cards.iter().find(|c| c.id == op.card_id);
    if op.kind == MutationKind::AddComment {
        let project_id = card_opt
            .map(|c| c.project_id.clone())
            .or_else(|| op.params["project_id"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let card_revision = card_opt.map(|c| c.revision).unwrap_or(0);
        let body = op.params["body"].as_str().unwrap_or("").to_owned();
        let comment_id = op.params["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| op.id.clone());
        // Emit `"body"` without `"message"` (`Event` uses `#[serde(alias = "body")]` on `message`).
        return Ok(json!({
            "id": comment_id,
            "project_id": project_id,
            "card_id": op.card_id,
            "sequence": u64::MAX,
            "kind": "comment.created",
            "actor": "user",
            "body": body,
            "data": {
                "body": body,
                "pending": true,
                "mutation_id": op.id,
            },
            "created_at": op.created_at,
            "card_revision": card_revision,
            "accepted": true,
            "request_key": op.id,
            "offline_queued": true,
        }));
    }

    let mut val = if let Some(card) = card_opt {
        serde_json::to_value(card)?
    } else {
        json!({
            "id": op.card_id,
            "number": 0,
            "stage": op.params.get("stage").and_then(Value::as_str).unwrap_or("backlog"),
            "revision": op.params.get("revision").and_then(Value::as_u64).unwrap_or(0),
        })
    };
    if let Some(obj) = val.as_object_mut() {
        obj.insert("accepted".into(), json!(true));
        obj.insert("request_key".into(), json!(op.id));
        obj.insert("offline_queued".into(), json!(true));
    }
    Ok(val)
}

/// Construct synthetic pending `Event` structs for offline `timeline.list` responses.
pub fn pending_comment_events(ops: &[Mutation], card_id: &str, snapshot: &Snapshot) -> Vec<Event> {
    let project_id = snapshot
        .cards
        .iter()
        .find(|c| c.id == card_id)
        .map(|c| c.project_id.clone())
        .unwrap_or_default();
    ops.iter()
        .filter(|m| m.card_id == card_id && m.kind == MutationKind::AddComment)
        .map(|m| {
            let body = m.params["body"].as_str().unwrap_or("").to_owned();
            let event_id = m.params["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| m.id.clone());
            Event {
                id: event_id,
                project_id: project_id.clone(),
                card_id: card_id.to_owned(),
                sequence: u64::MAX,
                kind: "comment.created".into(),
                actor: "user".into(),
                message: body.clone(),
                data: json!({
                    "body": body,
                    "pending": true,
                    "mutation_id": m.id,
                }),
                created_at: m.created_at,
            }
        })
        .collect()
}

/// Build an offline `card.resolve.preview` response from the cached + projected snapshot.
pub fn offline_resolve_preview(
    snapshot: &Snapshot,
    card_id: &str,
    host_name: &str,
) -> Result<Value> {
    let card = snapshot
        .cards
        .iter()
        .find(|c| c.id == card_id)
        .with_context(|| format!("Card '{card_id}' not found in cached snapshot"))?;
    let mut warnings = Vec::new();
    if snapshot
        .workflow_runs
        .iter()
        .any(|r| r.card_id.as_deref() == Some(card.id.as_str()) && r.active())
    {
        warnings.push("Active workflow run will be stopped".to_owned());
    }
    let agent_warning = agent_activity_warning(&card.id, &snapshot.terminals);
    let has_active_agent = agent_warning.is_some();
    if let Some(w) = agent_warning {
        warnings.push(w);
    }
    warnings.push(format!(
        "Offline: worktree changes could not be checked; Resolve will be applied when {host_name} reconnects"
    ));
    Ok(json!({
        "card_id": card.id,
        "revision": card.revision,
        "has_active_agent": has_active_agent,
        "warnings": warnings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Project, Workflow};

    fn base_snap() -> Snapshot {
        let mut s = Snapshot::default();
        s.projects.push(Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: vec![],
            id: "p1".into(),
            name: "Proj".into(),
            directory: "/tmp/proj".into(),
            workflow: Workflow::default(),
            target_branch: None,
            created_at: 1,
        });
        s.cards.push(Card {
            schedule_source: None,
            source_trust: Some("local".into()),
            hold_auto_merge: false,
            github_links: Vec::new(),
            created_by: String::new(),
            content: CardContent::Local,
            removed_at: None,
            pull_request: None,
            pull_request_history: Vec::new(),
            delivery_mode: None,
            workspace: None,
            workspace_history: Vec::new(),
            id: "c-existing".into(),
            project_id: "p1".into(),
            number: 1,
            board_order: Some(0),
            title: "Existing".into(),
            body: "Body".into(),
            context_notes: None,
            stage: "backlog".into(),
            revision: 3,
            attention: Some("waiting".into()),
            archived_at: None,
            archive: None,
            archive_error: None,
            worktree: Some("/tmp/wt".into()),
            branch: None,
            base_commit: None,
            human_gate: None,
            delivery: None,
            resolution: None,
            created_at: 1,
        });
        s
    }

    #[test]
    fn projects_all_mutation_kinds_and_preserves_base_revision() {
        let mut snap = base_snap();
        let ops = vec![
            Mutation {
                id: "m1".into(),
                host_id: "h1".into(),
                card_id: "c-new".into(),
                kind: MutationKind::CreateCard,
                params: json!({"id": "c-new", "project_id": "p1", "title": "New Card", "body": "Text"}),
                base: Default::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 10,
                attempts: 0,
            },
            Mutation {
                id: "m2".into(),
                host_id: "h1".into(),
                card_id: "c-existing".into(),
                kind: MutationKind::UpdateCard,
                params: json!({"card_id": "c-existing", "title": "Updated", "body": "NewBody"}),
                base: Default::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 11,
                attempts: 0,
            },
            Mutation {
                id: "m3".into(),
                host_id: "h1".into(),
                card_id: "c-existing".into(),
                kind: MutationKind::MoveCard,
                params: json!({"card_id": "c-existing", "stage": "develop"}),
                base: Default::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 12,
                attempts: 0,
            },
            Mutation {
                id: "m4".into(),
                host_id: "h1".into(),
                card_id: "c-existing".into(),
                kind: MutationKind::ResolveCard,
                params: json!({"card_id": "c-existing"}),
                base: Default::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Failed {
                    message: "oops".into(),
                },
                created_at: 13,
                attempts: 1,
            },
        ];

        apply(&mut snap, "h1", &ops);
        let new_c = snap.cards.iter().find(|c| c.id == "c-new").unwrap();
        assert_eq!(new_c.number, 0);
        assert_eq!(new_c.revision, 0);
        assert_eq!(new_c.stage, "backlog");
        assert_eq!(snap.pending_sync.get("c-new"), Some(&SyncMark::Pending));

        let ex = snap.cards.iter().find(|c| c.id == "c-existing").unwrap();
        assert_eq!(ex.title, "Updated");
        assert_eq!(ex.stage, "develop");
        assert!(ex.resolved());
        assert_eq!(ex.revision, 3);
        assert_eq!(snap.pending_sync.get("c-existing"), Some(&SyncMark::Failed));

        let preview = offline_resolve_preview(&snap, "c-existing", "Home Linux").unwrap();
        assert!(
            preview["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w.as_str().unwrap().contains("Home Linux"))
        );

        let comment_op = Mutation {
            id: "m5".into(),
            host_id: "h1".into(),
            card_id: "c-existing".into(),
            kind: MutationKind::AddComment,
            params: json!({"id": "ev123", "card_id": "c-existing", "body": "Offline comment"}),
            base: Default::default(),
            sent_params: Value::Null,
            retry_same_params: false,
            state: MutationState::Queued,
            created_at: 14,
            attempts: 0,
        };
        let receipt = synthetic_receipt(&comment_op, &base_snap(), &ops, "h1").unwrap();
        assert_eq!(receipt["offline_queued"], true);
        let decoded: Event = serde_json::from_value(receipt).unwrap();
        assert_eq!(decoded.id, "ev123");
        assert_eq!(decoded.sequence, u64::MAX);
    }
}
