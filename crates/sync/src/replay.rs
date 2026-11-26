//! FIFO outbox replay against a live remote host.
use crate::{
    merge::{FieldMergeOutcome, classify_card_update},
    outbox::{Mutation, MutationKind, MutationState, Outbox},
};
use anyhow::Result;
use canban_types::{Card, Snapshot};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Maximum immediate `revision_conflict` refresh-and-rebuild retries for a single mutation
/// within one drain pass before deferring the remaining `Queued` op to a subsequent drain.
pub const MAX_REVISION_REBUILDS_PER_DRAIN: u32 = 3;

/// Returns true when a server rejection indicates an offline `card.move` targeted a
/// `before_card_id` peer that has since moved columns, been resolved, or been archived.
pub fn is_drop_target_changed_error(msg: &str) -> bool {
    msg.contains("Drop target changed")
}

/// Returns true when a non-connection replay error requires fetching a fresh host snapshot
/// before deciding whether to rebuild, retry, or fail the mutation.
pub fn needs_snapshot_refresh_on_error(msg: &str) -> bool {
    msg.contains("revision_conflict")
        || msg.contains("request_key_conflict")
        || is_drop_target_changed_error(msg)
}

/// Host RPC + snapshot transport used by [`drain`].
pub trait Transport {
    fn call(&self, method: &str, params: Value) -> Result<Value>;
    fn refresh_snapshot(&self) -> Result<Snapshot>;
    fn is_connection_error(&self, err: &anyhow::Error) -> bool;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub applied: usize,
    pub conflicts: usize,
    pub failed: usize,
    pub aborted_by_connection: bool,
}

enum PrepareResult {
    Send(Value),
    AlreadySatisfied,
    Conflict(Vec<String>),
    Failed(String),
}

fn with_full_card_view(kind: MutationKind, mut p: Value) -> Value {
    if matches!(
        kind,
        MutationKind::CreateCard
            | MutationKind::UpdateCard
            | MutationKind::MoveCard
            | MutationKind::ResolveCard
    ) && let Some(obj) = p.as_object_mut()
    {
        // Presentation fields (`view`/`fields`) are stripped before server `request_key`
        // fingerprinting, so requesting `view: "full"` stays idempotent even on retries
        // and guarantees `upsert_card_into_snapshot` receives the full authoritative `Card`.
        obj.remove("fields");
        obj.insert("view".into(), Value::String("full".into()));
    }
    p
}

fn prepare_params(op: &Mutation, snapshot: &Snapshot) -> PrepareResult {
    if op.retry_same_params && !op.sent_params.is_null() {
        return PrepareResult::Send(with_full_card_view(op.kind, op.sent_params.clone()));
    }
    let strip_host_and_force = |mut p: Value| {
        if let Some(obj) = p.as_object_mut() {
            obj.remove("host_id");
            obj.remove("force");
            obj.insert("request_key".into(), Value::String(op.id.clone()));
        }
        with_full_card_view(op.kind, p)
    };
    match op.kind {
        MutationKind::CreateCard => PrepareResult::Send(strip_host_and_force(op.params.clone())),
        MutationKind::AddComment => PrepareResult::Send(strip_host_and_force(op.params.clone())),
        MutationKind::SyncGlobalEnv => PrepareResult::Send(op.params.clone()),
        MutationKind::UpdateCard => {
            let Some(server) = snapshot.cards.iter().find(|c| c.id == op.card_id) else {
                return PrepareResult::Failed("Card no longer exists on host".into());
            };
            if !server.active() {
                return PrepareResult::Failed(
                    "Card is archived or removed; restore it before syncing edits".into(),
                );
            }
            let force = op.params["force"] == true;
            let mine_title = op.params.get("title").and_then(Value::as_str);
            let mine_body = op.params.get("body").and_then(Value::as_str);
            let (apply_title, apply_body) = if force {
                (mine_title.map(str::to_owned), mine_body.map(str::to_owned))
            } else {
                match classify_card_update(&op.base, mine_title, mine_body, server) {
                    FieldMergeOutcome::NoOp => return PrepareResult::AlreadySatisfied,
                    FieldMergeOutcome::Conflict { fields, .. } => {
                        return PrepareResult::Conflict(fields);
                    }
                    FieldMergeOutcome::Apply { title, body } => (title, body),
                }
            };
            let mut p = json!({
                "card_id": op.card_id,
                "revision": server.revision,
                "request_key": op.id,
                "view": "full",
            });
            if let Some(t) = apply_title {
                p["title"] = Value::String(t);
            }
            if let Some(b) = apply_body {
                p["body"] = Value::String(b);
            }
            PrepareResult::Send(p)
        }
        MutationKind::MoveCard => {
            let Some(server) = snapshot.cards.iter().find(|c| c.id == op.card_id) else {
                return PrepareResult::Failed("Card no longer exists on host".into());
            };
            if !server.active() {
                return PrepareResult::Failed("Card is archived or removed".into());
            }
            let Some(target) = op.params.get("stage").and_then(Value::as_str) else {
                return PrepareResult::Failed("Missing target stage".into());
            };
            if server.stage == target {
                return PrepareResult::AlreadySatisfied;
            }
            let mut p = strip_host_and_force(op.params.clone());
            p["revision"] = json!(server.revision);
            // If an offline ranked move referenced a `before_card_id` peer that is no longer
            // an active, unresolved card in the target column, fall back to appending to the
            // column rather than permanently failing the stage move.
            if let Some(before_id) = p.get("before_card_id").and_then(Value::as_str) {
                let target_peer_valid = snapshot.cards.iter().any(|c| {
                    c.id == before_id
                        && c.project_id == server.project_id
                        && c.active()
                        && !c.resolved()
                        && c.stage == target
                        && c.id != server.id
                });
                if !target_peer_valid && let Some(obj) = p.as_object_mut() {
                    obj.remove("before_card_id");
                }
            }
            PrepareResult::Send(p)
        }
        MutationKind::ResolveCard => {
            let Some(server) = snapshot.cards.iter().find(|c| c.id == op.card_id) else {
                return PrepareResult::Failed("Card no longer exists on host".into());
            };
            if !server.active() {
                return PrepareResult::Failed("Card is archived or removed".into());
            }
            if server.resolved() {
                return PrepareResult::AlreadySatisfied;
            }
            let mut p = strip_host_and_force(op.params.clone());
            if let Some(obj) = p.as_object_mut() {
                // `card.resolve` checks revision only when supplied; omitting avoids stale-rev rejection.
                obj.remove("revision");
            }
            PrepareResult::Send(p)
        }
    }
}

fn card_already_matches_after_key_conflict(op: &Mutation, snapshot: &Snapshot) -> bool {
    let Some(card) = snapshot.cards.iter().find(|c| c.id == op.card_id) else {
        return false;
    };
    match op.kind {
        MutationKind::CreateCard => true,
        MutationKind::UpdateCard => {
            let title_ok = op
                .params
                .get("title")
                .and_then(Value::as_str)
                .is_none_or(|t| card.title == t.trim());
            let body_ok = op
                .params
                .get("body")
                .and_then(Value::as_str)
                .is_none_or(|b| card.body == b);
            title_ok && body_ok
        }
        MutationKind::MoveCard => op
            .params
            .get("stage")
            .and_then(Value::as_str)
            .is_some_and(|s| card.stage == s),
        MutationKind::ResolveCard => card.resolved(),
        MutationKind::AddComment | MutationKind::SyncGlobalEnv => false,
    }
}

fn upsert_card_into_snapshot(snapshot: &mut Snapshot, op: &Mutation, reply: &Value) {
    if let Ok(card) = serde_json::from_value::<Card>(reply.clone()) {
        if let Some(existing) = snapshot.cards.iter_mut().find(|c| c.id == card.id) {
            *existing = card;
        } else {
            snapshot.cards.push(card);
        }
        return;
    }
    // Fallback for compact write receipts (`id`, `number`, `title`, `stage`, `revision`) so
    // subsequent queued FIFO mutations on the same Card see the updated revision & fields.
    let Some(id) = reply.get("id").and_then(Value::as_str) else {
        return;
    };
    if let Some(existing) = snapshot.cards.iter_mut().find(|c| c.id == id) {
        if let Some(rev) = reply.get("revision").and_then(Value::as_u64) {
            existing.revision = rev;
        }
        if let Some(title) = reply.get("title").and_then(Value::as_str) {
            existing.title = title.to_owned();
        }
        if let Some(stage) = reply.get("stage").and_then(Value::as_str) {
            existing.stage = stage.to_owned();
        }
        if let Some(body) = reply
            .get("body")
            .and_then(Value::as_str)
            .or_else(|| op.params.get("body").and_then(Value::as_str))
        {
            existing.body = body.to_owned();
        }
    } else if op.kind == MutationKind::CreateCard {
        let project_id = reply
            .get("project_id")
            .and_then(Value::as_str)
            .or_else(|| op.params.get("project_id").and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();
        let title = reply
            .get("title")
            .and_then(Value::as_str)
            .or_else(|| op.params.get("title").and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();
        let body = reply
            .get("body")
            .and_then(Value::as_str)
            .or_else(|| op.params.get("body").and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();
        let stage = reply
            .get("stage")
            .and_then(Value::as_str)
            .or_else(|| op.params.get("stage").and_then(Value::as_str))
            .unwrap_or("backlog")
            .to_owned();
        let revision = reply.get("revision").and_then(Value::as_u64).unwrap_or(1);
        let number = reply.get("number").and_then(Value::as_u64).unwrap_or(0);
        let fallback_card = json!({
            "id": id,
            "project_id": project_id,
            "number": number,
            "title": title,
            "body": body,
            "stage": stage,
            "revision": revision,
            "created_at": op.created_at,
            "created_by": "user",
        });
        if let Ok(card) = serde_json::from_value::<Card>(fallback_card) {
            snapshot.cards.push(card);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepAction {
    Idle,
    Send {
        mutation_id: String,
        method: &'static str,
        sent_params: Value,
        was_retry_same_params: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishOutcome {
    Continue,
    RetryImmediate,
    Deferred,
    StopConnection,
}

/// Inspect `outbox` and prepare the next non-blocked operation without holding locks across I/O.
pub fn prepare_next_step(
    outbox: &mut Outbox,
    snapshot: &Snapshot,
    blocked: &mut BTreeSet<String>,
    report: &mut DrainReport,
) -> StepAction {
    let mut idx = 0;
    while let Some(op) = outbox.ops.get(idx) {
        let block_key = if op.kind == MutationKind::SyncGlobalEnv {
            "__global_env__".to_owned()
        } else {
            op.card_id.clone()
        };
        if blocked.contains(&block_key) {
            idx += 1;
            continue;
        }
        match &outbox.ops[idx].state {
            MutationState::Conflict { .. } | MutationState::Failed { .. } => {
                blocked.insert(block_key);
                idx += 1;
                continue;
            }
            MutationState::Queued | MutationState::InFlight => {}
        }

        match prepare_params(&outbox.ops[idx], snapshot) {
            PrepareResult::AlreadySatisfied => {
                outbox.ops.remove(idx);
                let _ = outbox.save();
                report.applied += 1;
            }
            PrepareResult::Conflict(fields) => {
                outbox.ops[idx].state = MutationState::Conflict { fields };
                let _ = outbox.save();
                blocked.insert(block_key);
                report.conflicts += 1;
                idx += 1;
            }
            PrepareResult::Failed(message) => {
                outbox.ops[idx].state = MutationState::Failed { message };
                let _ = outbox.save();
                blocked.insert(block_key);
                report.failed += 1;
                idx += 1;
            }
            PrepareResult::Send(sent_params) => {
                let prev = outbox.ops[idx].clone();
                let mutation_id = prev.id.clone();
                let method = prev.kind.as_method();
                let was_retry_same_params = prev.retry_same_params;
                outbox.ops[idx].sent_params = sent_params.clone();
                outbox.ops[idx].retry_same_params = false;
                outbox.ops[idx].state = MutationState::InFlight;
                outbox.ops[idx].attempts = outbox.ops[idx].attempts.saturating_add(1);
                if outbox.save().is_err() {
                    outbox.ops[idx] = prev;
                    return StepAction::Idle;
                }
                return StepAction::Send {
                    mutation_id,
                    method,
                    sent_params,
                    was_retry_same_params,
                };
            }
        }
    }
    StepAction::Idle
}

/// Apply the result of executing `mutation_id` (outside the outbox lock) back into `outbox`.
#[allow(clippy::too_many_arguments)]
pub fn finish_step(
    outbox: &mut Outbox,
    snapshot: &mut Snapshot,
    mutation_id: &str,
    result: Result<Value>,
    was_retry_same_params: bool,
    is_conn_err: bool,
    refreshed_snapshot: Option<Snapshot>,
    allow_revision_rebuild: bool,
    blocked: &mut BTreeSet<String>,
    report: &mut DrainReport,
) -> FinishOutcome {
    let Some(pos) = outbox.ops.iter().position(|m| m.id == mutation_id) else {
        return FinishOutcome::Continue;
    };
    let block_key = if outbox.ops[pos].kind == MutationKind::SyncGlobalEnv {
        "__global_env__".to_owned()
    } else {
        outbox.ops[pos].card_id.clone()
    };
    match result {
        Ok(reply) => {
            upsert_card_into_snapshot(snapshot, &outbox.ops[pos], &reply);
            outbox.ops.remove(pos);
            let _ = outbox.save();
            report.applied += 1;
            FinishOutcome::Continue
        }
        Err(err) => {
            let msg = format!("{err:#}");
            if is_conn_err {
                outbox.ops[pos].state = MutationState::Queued;
                // A `revision_conflict` or `Drop target changed` proves the server rejected our
                // stale request without committing `request_key`, so when the follow-up `app.state`
                // refresh fails with a transport error we must NOT preserve the stale `sent_params`
                // across reconnect; let the next drain re-classify against the fresh snapshot.
                let stale_uncommitted = msg.contains("revision_conflict")
                    || (outbox.ops[pos].kind == MutationKind::MoveCard
                        && is_drop_target_changed_error(&msg));
                outbox.ops[pos].retry_same_params = !stale_uncommitted;
                let _ = outbox.save();
                report.aborted_by_connection = true;
                return FinishOutcome::StopConnection;
            }
            if let Some(fresh) = refreshed_snapshot {
                *snapshot = fresh;
                let stale_drop_target = outbox.ops[pos].kind == MutationKind::MoveCard
                    && is_drop_target_changed_error(&msg);
                if msg.contains("revision_conflict") || stale_drop_target {
                    // A `revision_conflict` or `Drop target changed` proves the server never
                    // committed `request_key` (even when `was_retry_same_params` was true after a
                    // transport drop), so refreshing and re-classifying / rebuilding is safe.
                    let _ = was_retry_same_params;
                    outbox.ops[pos].retry_same_params = false;
                    if stale_drop_target && let Some(obj) = outbox.ops[pos].params.as_object_mut() {
                        obj.remove("before_card_id");
                    }
                    if allow_revision_rebuild {
                        outbox.ops[pos].state = MutationState::Queued;
                        let _ = outbox.save();
                        return FinishOutcome::RetryImmediate;
                    }
                    // Immediate rebuild budget for this drain pass is exhausted; re-evaluate
                    // against the freshly fetched snapshot so genuine conflicts, no-ops, or
                    // archived/deleted cards resolve immediately instead of becoming a spurious
                    // `Failed("revision_conflict")`, while still-valid edits stay `Queued` and
                    // defer to the next drain pass.
                    match prepare_params(&outbox.ops[pos], snapshot) {
                        PrepareResult::AlreadySatisfied => {
                            outbox.ops.remove(pos);
                            let _ = outbox.save();
                            report.applied += 1;
                            return FinishOutcome::Continue;
                        }
                        PrepareResult::Conflict(fields) => {
                            outbox.ops[pos].state = MutationState::Conflict { fields };
                            let _ = outbox.save();
                            blocked.insert(block_key);
                            report.conflicts += 1;
                            return FinishOutcome::Continue;
                        }
                        PrepareResult::Failed(message) => {
                            outbox.ops[pos].state = MutationState::Failed { message };
                            let _ = outbox.save();
                            blocked.insert(block_key);
                            report.failed += 1;
                            return FinishOutcome::Continue;
                        }
                        PrepareResult::Send(_) => {
                            outbox.ops[pos].state = MutationState::Queued;
                            let _ = outbox.save();
                            blocked.insert(block_key);
                            return FinishOutcome::Deferred;
                        }
                    }
                }
                if msg.contains("request_key_conflict")
                    && card_already_matches_after_key_conflict(&outbox.ops[pos], snapshot)
                {
                    outbox.ops.remove(pos);
                    let _ = outbox.save();
                    report.applied += 1;
                    return FinishOutcome::Continue;
                }
            }
            outbox.ops[pos].state = MutationState::Failed { message: msg };
            let _ = outbox.save();
            blocked.insert(block_key);
            report.failed += 1;
            FinishOutcome::Continue
        }
    }
}

/// Replay queued mutations in FIFO order; blocked cards do not stall other cards.
pub fn drain(
    outbox: &mut Outbox,
    snapshot: &mut Snapshot,
    transport: &dyn Transport,
) -> DrainReport {
    let mut report = DrainReport::default();
    let mut blocked = BTreeSet::new();
    let mut rebuild_counts: BTreeMap<String, u32> = BTreeMap::new();

    while let StepAction::Send {
        mutation_id,
        method,
        sent_params,
        was_retry_same_params,
    } = prepare_next_step(outbox, snapshot, &mut blocked, &mut report)
    {
        let allow_rebuild = rebuild_counts.get(&mutation_id).copied().unwrap_or(0)
            < MAX_REVISION_REBUILDS_PER_DRAIN;
        let result = transport.call(method, sent_params);
        let (is_conn_err, refreshed) = match &result {
            Ok(_) => (false, None),
            Err(err) => {
                let mut conn = transport.is_connection_error(err);
                let msg = format!("{err:#}");
                let need_refresh = !conn && needs_snapshot_refresh_on_error(&msg);
                let snap = if need_refresh {
                    match transport.refresh_snapshot() {
                        Ok(s) => Some(s),
                        Err(refresh_err) => {
                            if transport.is_connection_error(&refresh_err) {
                                conn = true;
                            }
                            None
                        }
                    }
                } else {
                    None
                };
                (conn, snap)
            }
        };
        let outcome = finish_step(
            outbox,
            snapshot,
            &mutation_id,
            result,
            was_retry_same_params,
            is_conn_err,
            refreshed,
            allow_rebuild,
            &mut blocked,
            &mut report,
        );
        match outcome {
            FinishOutcome::Continue | FinishOutcome::Deferred => {}
            FinishOutcome::RetryImmediate => {
                *rebuild_counts.entry(mutation_id).or_insert(0) += 1;
            }
            FinishOutcome::StopConnection => break,
        }
    }
    report
}
