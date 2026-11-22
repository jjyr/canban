//! FIFO outbox replay against a live remote host.
use crate::{
    merge::{FieldMergeOutcome, classify_card_update},
    outbox::{Mutation, MutationKind, MutationState, Outbox},
};
use anyhow::Result;
use canban_types::{Card, Snapshot};
use serde_json::{Value, json};
use std::collections::BTreeSet;

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

fn prepare_params(op: &Mutation, snapshot: &Snapshot) -> PrepareResult {
    if op.retry_same_params && !op.sent_params.is_null() {
        return PrepareResult::Send(op.sent_params.clone());
    }
    let strip_host_and_force = |mut p: Value| {
        if let Some(obj) = p.as_object_mut() {
            obj.remove("host_id");
            obj.remove("force");
            obj.insert("request_key".into(), Value::String(op.id.clone()));
        }
        p
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
            });
            if let Some(view) = op.params.get("view") {
                p["view"] = view.clone();
            } else if let Some(fields) = op.params.get("fields") {
                p["fields"] = fields.clone();
            }
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

fn upsert_card_into_snapshot(snapshot: &mut Snapshot, reply: &Value) {
    if let Ok(card) = serde_json::from_value::<Card>(reply.clone()) {
        if let Some(existing) = snapshot.cards.iter_mut().find(|c| c.id == card.id) {
            *existing = card;
        } else {
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
            upsert_card_into_snapshot(snapshot, &reply);
            outbox.ops.remove(pos);
            let _ = outbox.save();
            report.applied += 1;
            FinishOutcome::Continue
        }
        Err(err) => {
            if is_conn_err {
                outbox.ops[pos].state = MutationState::Queued;
                outbox.ops[pos].retry_same_params = true;
                let _ = outbox.save();
                report.aborted_by_connection = true;
                return FinishOutcome::StopConnection;
            }
            let msg = format!("{err:#}");
            if let Some(fresh) = refreshed_snapshot {
                *snapshot = fresh;
                if msg.contains("revision_conflict")
                    && !was_retry_same_params
                    && allow_revision_rebuild
                {
                    outbox.ops[pos].state = MutationState::Queued;
                    let _ = outbox.save();
                    return FinishOutcome::RetryImmediate;
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
    let mut rebuilt_ids = BTreeSet::new();

    while let StepAction::Send {
        mutation_id,
        method,
        sent_params,
        was_retry_same_params,
    } = prepare_next_step(outbox, snapshot, &mut blocked, &mut report)
    {
        let allow_rebuild = !rebuilt_ids.contains(&mutation_id);
        let result = transport.call(method, sent_params);
        let (is_conn_err, refreshed) = match &result {
            Ok(_) => (false, None),
            Err(err) => {
                let conn = transport.is_connection_error(err);
                let msg = format!("{err:#}");
                let need_refresh = !conn
                    && ((msg.contains("revision_conflict")
                        && !was_retry_same_params
                        && allow_rebuild)
                        || msg.contains("request_key_conflict"));
                let snap = if need_refresh {
                    transport.refresh_snapshot().ok()
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
            FinishOutcome::Continue => {}
            FinishOutcome::RetryImmediate => {
                rebuilt_ids.insert(mutation_id);
            }
            FinishOutcome::StopConnection => break,
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::UpdateBase;
    use anyhow::anyhow;
    use canban_types::{CardContent, Project, Workflow};
    use std::cell::RefCell;

    fn make_card(id: &str, title: &str, stage: &str, revision: u64) -> Card {
        Card {
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
            id: id.into(),
            project_id: "p1".into(),
            number: 1,
            board_order: Some(0),
            title: title.into(),
            body: "Body".into(),
            context_notes: None,
            stage: stage.into(),
            revision,
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
            created_at: 1,
        }
    }

    fn snap_with_cards(cards: Vec<Card>) -> Snapshot {
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
            directory: "/tmp/p".into(),
            workflow: Workflow::default(),
            target_branch: None,
            created_at: 1,
        });
        s.cards = cards;
        s
    }

    type MockHandler = Box<dyn Fn(&str, &Value) -> Result<Value>>;

    struct MockTransport {
        calls: RefCell<Vec<(String, Value)>>,
        handler: MockHandler,
        refresh: Box<dyn Fn() -> Result<Snapshot>>,
    }

    impl Transport for MockTransport {
        fn call(&self, method: &str, params: Value) -> Result<Value> {
            self.calls
                .borrow_mut()
                .push((method.into(), params.clone()));
            (self.handler)(method, &params)
        }
        fn refresh_snapshot(&self) -> Result<Snapshot> {
            (self.refresh)()
        }
        fn is_connection_error(&self, err: &anyhow::Error) -> bool {
            err.downcast_ref::<std::io::Error>().is_some()
        }
    }

    #[test]
    fn transport_error_retries_with_identical_sent_params() {
        let dir = tempfile::tempdir().unwrap();
        let mut outbox = Outbox::load(dir.path().join("o.json"));
        outbox
            .enqueue(Mutation {
                id: "m1".into(),
                host_id: "h1".into(),
                card_id: "c1".into(),
                kind: MutationKind::MoveCard,
                params: json!({"card_id": "c1", "revision": 1, "stage": "develop"}),
                base: UpdateBase::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 1,
                attempts: 0,
            })
            .unwrap();

        let mut snap = snap_with_cards(vec![make_card("c1", "T", "backlog", 2)]);
        let transport_drop = MockTransport {
            calls: RefCell::new(vec![]),
            handler: Box::new(|_, _| {
                Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "drop").into())
            }),
            refresh: Box::new(|| unreachable!()),
        };
        let rep = drain(&mut outbox, &mut snap, &transport_drop);
        assert!(rep.aborted_by_connection);
        assert_eq!(outbox.ops.len(), 1);
        assert!(outbox.ops[0].retry_same_params);
        let first_sent = transport_drop.calls.borrow()[0].1.clone();
        assert_eq!(first_sent["revision"], 2);
        assert_eq!(first_sent["request_key"], "m1");

        // Even if local snapshot advances, retry uses identical `sent_params`.
        let mut snap2 = snap_with_cards(vec![make_card("c1", "T", "backlog", 99)]);
        let transport_ok = MockTransport {
            calls: RefCell::new(vec![]),
            handler: Box::new(|_, p| Ok(json!({"id": "c1", "revision": 3, "stage": p["stage"]}))),
            refresh: Box::new(|| unreachable!()),
        };
        let rep2 = drain(&mut outbox, &mut snap2, &transport_ok);
        assert_eq!(rep2.applied, 1);
        assert!(outbox.is_empty());
        assert_eq!(transport_ok.calls.borrow()[0].1, first_sent);
    }

    #[test]
    fn revision_error_rebuilds_params_with_fresh_revision_and_same_request_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut outbox = Outbox::load(dir.path().join("o.json"));
        outbox
            .enqueue(Mutation {
                id: "m-mv".into(),
                host_id: "h1".into(),
                card_id: "c1".into(),
                kind: MutationKind::MoveCard,
                params: json!({"card_id": "c1", "revision": 1, "stage": "develop"}),
                base: UpdateBase::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 1,
                attempts: 0,
            })
            .unwrap();
        let mut snap = snap_with_cards(vec![make_card("c1", "T", "backlog", 1)]);
        let attempt = RefCell::new(0);
        let transport = MockTransport {
            calls: RefCell::new(vec![]),
            handler: Box::new(move |_, p| {
                let mut n = attempt.borrow_mut();
                *n += 1;
                if *n == 1 {
                    Err(anyhow!("revision_conflict: expected 1"))
                } else {
                    assert_eq!(p["revision"], 4);
                    assert_eq!(p["request_key"], "m-mv");
                    Ok(serde_json::to_value(make_card("c1", "T", "develop", 5)).unwrap())
                }
            }),
            refresh: Box::new(|| Ok(snap_with_cards(vec![make_card("c1", "T", "backlog", 4)]))),
        };
        let rep = drain(&mut outbox, &mut snap, &transport);
        assert_eq!(rep.applied, 1);
        assert!(outbox.is_empty());
    }

    #[test]
    fn create_then_move_ordering_and_blocked_card_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let mut outbox = Outbox::load(dir.path().join("o.json"));
        outbox
            .enqueue(Mutation {
                id: "m-missing".into(),
                host_id: "h1".into(),
                card_id: "c-gone".into(),
                kind: MutationKind::MoveCard,
                params: json!({"card_id": "c-gone", "stage": "develop"}),
                base: UpdateBase::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 1,
                attempts: 0,
            })
            .unwrap();
        outbox
            .enqueue(Mutation {
                id: "m-missing-comment".into(),
                host_id: "h1".into(),
                card_id: "c-gone".into(),
                kind: MutationKind::AddComment,
                params: json!({"id": "ev-gone", "card_id": "c-gone", "body": "blocked"}),
                base: UpdateBase::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 2,
                attempts: 0,
            })
            .unwrap();
        outbox
            .enqueue(Mutation {
                id: "m-create".into(),
                host_id: "h1".into(),
                card_id: "c-new".into(),
                kind: MutationKind::CreateCard,
                params: json!({"id": "c-new", "project_id": "p1", "title": "Fresh", "body": ""}),
                base: UpdateBase::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 3,
                attempts: 0,
            })
            .unwrap();
        outbox
            .enqueue(Mutation {
                id: "m-move".into(),
                host_id: "h1".into(),
                card_id: "c-new".into(),
                kind: MutationKind::MoveCard,
                params: json!({"card_id": "c-new", "stage": "develop"}),
                base: UpdateBase::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 4,
                attempts: 0,
            })
            .unwrap();

        let mut snap = snap_with_cards(vec![]);
        let transport = MockTransport {
            calls: RefCell::new(vec![]),
            handler: Box::new(|method, p| match method {
                "card.create" => Ok(serde_json::to_value(make_card(
                    p["id"].as_str().unwrap(),
                    "Fresh",
                    "backlog",
                    1,
                ))
                .unwrap()),
                "card.move" => {
                    assert_eq!(p["revision"], 1);
                    Ok(serde_json::to_value(make_card("c-new", "Fresh", "develop", 2)).unwrap())
                }
                other => panic!("unexpected call {other}"),
            }),
            refresh: Box::new(|| Ok(snap_with_cards(vec![]))),
        };
        let rep = drain(&mut outbox, &mut snap, &transport);
        assert_eq!(rep.failed, 1);
        assert_eq!(rep.applied, 2);
        // `c-gone` move failed and blocked `c-gone`'s comment, while `c-new` create+move proceeded.
        assert_eq!(outbox.len(), 2);
        assert!(matches!(outbox.ops[0].state, MutationState::Failed { .. }));
        assert_eq!(outbox.ops[1].id, "m-missing-comment");
        assert_eq!(outbox.ops[1].state, MutationState::Queued);
    }

    #[test]
    fn failed_global_env_sync_does_not_block_card_ops() {
        let dir = tempfile::tempdir().unwrap();
        let mut outbox = Outbox::load(dir.path().join("o.json"));
        outbox
            .enqueue(Mutation {
                id: "m-genv".into(),
                host_id: "h1".into(),
                card_id: String::new(),
                kind: MutationKind::SyncGlobalEnv,
                params: json!({"generation": 1}),
                base: UpdateBase::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 1,
                attempts: 0,
            })
            .unwrap();
        outbox
            .enqueue(Mutation {
                id: "m-create".into(),
                host_id: "h1".into(),
                card_id: "c-ok".into(),
                kind: MutationKind::CreateCard,
                params: json!({"id": "c-ok", "project_id": "p1", "title": "Ok", "body": ""}),
                base: UpdateBase::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: 2,
                attempts: 0,
            })
            .unwrap();
        let mut snap = snap_with_cards(vec![]);
        let transport = MockTransport {
            calls: RefCell::new(vec![]),
            handler: Box::new(|method, p| match method {
                "env.global.sync" => Err(anyhow!("sync failed")),
                "card.create" => Ok(serde_json::to_value(make_card(
                    p["id"].as_str().unwrap(),
                    "Ok",
                    "backlog",
                    1,
                ))
                .unwrap()),
                other => panic!("unexpected call {other}"),
            }),
            refresh: Box::new(|| Ok(snap_with_cards(vec![]))),
        };
        let rep = drain(&mut outbox, &mut snap, &transport);
        assert_eq!(rep.failed, 1);
        assert_eq!(rep.applied, 1);
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox.ops[0].kind, MutationKind::SyncGlobalEnv);
    }
}
