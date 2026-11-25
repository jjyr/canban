//! Persistent FIFO mutation outbox per remote host with create+update coalescing.
use anyhow::{Result, ensure};
use canban_types::{FP_OUTBOX_MID_WRITE_TRUNCATE, Failpoints};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_OUTBOX_OPS: usize = 500;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    CreateCard,
    UpdateCard,
    MoveCard,
    AddComment,
    ResolveCard,
    SyncGlobalEnv,
}

impl MutationKind {
    pub fn as_method(self) -> &'static str {
        match self {
            Self::CreateCard => "card.create",
            Self::UpdateCard => "card.update",
            Self::MoveCard => "card.move",
            Self::AddComment => "comment.add",
            Self::ResolveCard => "card.resolve",
            Self::SyncGlobalEnv => "env.global.sync",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CreateCard => "Create Card",
            Self::UpdateCard => "Edit Card",
            Self::MoveCard => "Move Card",
            Self::AddComment => "Add Comment",
            Self::ResolveCard => "Resolve Card",
            Self::SyncGlobalEnv => "Sync Global Environment",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateBase {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MutationState {
    Queued,
    InFlight,
    Conflict { fields: Vec<String> },
    Failed { message: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Mutation {
    pub id: String,
    pub host_id: String,
    pub card_id: String,
    pub kind: MutationKind,
    pub params: Value,
    #[serde(default)]
    pub base: UpdateBase,
    #[serde(default)]
    pub sent_params: Value,
    /// True after a transport-level failure on `sent_params`, requiring identical params on retry.
    #[serde(default)]
    pub retry_same_params: bool,
    pub state: MutationState,
    pub created_at: u64,
    #[serde(default)]
    pub attempts: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct OutboxFile {
    #[serde(default)]
    ops: Vec<Mutation>,
}

#[derive(Debug)]
pub struct Outbox {
    path: PathBuf,
    durability: canban_context::FsDurability,
    failpoints: Option<Arc<Failpoints>>,
    pub ops: Vec<Mutation>,
}

impl Outbox {
    pub fn load(path: PathBuf) -> Self {
        Self::load_with_durability(path, canban_context::FsDurability::Full)
    }

    pub fn load_with_durability(path: PathBuf, durability: canban_context::FsDurability) -> Self {
        Self::load_with_durability_and_failpoints(path, durability, None)
    }

    pub fn load_with_durability_and_failpoints(
        path: PathBuf,
        durability: canban_context::FsDurability,
        failpoints: Option<Arc<Failpoints>>,
    ) -> Self {
        let mut ops = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<OutboxFile>(&bytes) {
                Ok(file) => file.ops,
                Err(_) => {
                    // Quarantine corrupt/truncated outbox JSON so reload recovers cleanly
                    // without losing forensic bytes or blocking subsequent persistence.
                    let quarantine = Self::quarantine_path(&path);
                    let _ = std::fs::rename(&path, quarantine);
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        // Recover from a crash mid-attempt: keep `sent_params` intact so retries remain idempotent.
        for op in &mut ops {
            if matches!(op.state, MutationState::InFlight) {
                op.state = MutationState::Queued;
                if op.attempts > 0 && !op.sent_params.is_null() {
                    op.retry_same_params = true;
                }
            }
        }
        Self {
            path,
            durability,
            failpoints,
            ops,
        }
    }

    /// Attaches an optional per-instance [`Failpoints`] registry (builder style).
    pub fn with_failpoints(mut self, failpoints: Option<Arc<Failpoints>>) -> Self {
        self.failpoints = failpoints;
        self
    }

    /// Replaces the optional per-instance [`Failpoints`] registry on this outbox.
    pub fn set_failpoints(&mut self, failpoints: Option<Arc<Failpoints>>) {
        self.failpoints = failpoints;
    }

    /// Returns the quarantine path used when a corrupt or torn outbox file is detected on load.
    pub fn quarantine_path(path: &Path) -> PathBuf {
        let mut raw = path.as_os_str().to_os_string();
        raw.push(".corrupt");
        PathBuf::from(raw)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn save(&self) -> Result<()> {
        if self.ops.is_empty() {
            if self.path.exists() {
                std::fs::remove_file(&self.path)?;
                if self.durability == canban_context::FsDurability::Full
                    && let Some(parent) = self.path.parent()
                    && let Ok(dir) = std::fs::File::open(parent)
                {
                    let _ = dir.sync_all();
                }
            }
            return Ok(());
        }
        let file = OutboxFile {
            ops: self.ops.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file)?;
        if let Err(msg) =
            Failpoints::check_opt(self.failpoints.as_ref(), FP_OUTBOX_MID_WRITE_TRUNCATE)
        {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Write a strict non-empty truncated prefix directly to the outbox path
            // to simulate a torn partial write on crash, then return an io::Error.
            let prefix_len = (bytes.len() / 2).max(1).min(bytes.len().saturating_sub(1));
            std::fs::write(&self.path, &bytes[..prefix_len])?;
            return Err(std::io::Error::other(format!(
                "failpoint[{FP_OUTBOX_MID_WRITE_TRUNCATE}]: {msg}"
            ))
            .into());
        }
        canban_context::atomic_with_durability(&self.path, &bytes, self.durability)
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn pending_count(&self) -> usize {
        self.ops.len()
    }

    pub fn conflict_count(&self) -> usize {
        self.ops
            .iter()
            .filter(|m| {
                matches!(
                    m.state,
                    MutationState::Conflict { .. } | MutationState::Failed { .. }
                )
            })
            .count()
    }

    pub fn blocked_cards(&self) -> BTreeSet<String> {
        self.ops
            .iter()
            .filter(|m| {
                matches!(
                    m.state,
                    MutationState::Conflict { .. } | MutationState::Failed { .. }
                )
            })
            .map(|m| {
                if m.kind == MutationKind::SyncGlobalEnv {
                    "__global_env__".to_owned()
                } else {
                    m.card_id.clone()
                }
            })
            .collect()
    }

    /// Enqueue `mutation`, coalescing an update into a queued `CreateCard` for the same Card.
    /// Duplicate `mutation.id` returns the existing op without adding a second copy.
    pub fn enqueue(&mut self, mutation: Mutation) -> Result<(Mutation, bool)> {
        if let Some(existing) = self.ops.iter().find(|m| m.id == mutation.id) {
            return Ok((existing.clone(), false));
        }
        let prev_ops = self.ops.clone();
        let save_or_rollback = |this: &mut Self| -> Result<()> {
            if let Err(err) = this.save() {
                this.ops = prev_ops;
                return Err(err);
            }
            Ok(())
        };
        if mutation.kind == MutationKind::SyncGlobalEnv {
            // Explicit sync retries clear prior failed/conflict SyncGlobalEnv entries so
            // `"__global_env__"` is unblocked before queuing/coalescing the next attempt.
            self.ops.retain(|m| {
                !(m.kind == MutationKind::SyncGlobalEnv
                    && matches!(
                        m.state,
                        MutationState::Failed { .. } | MutationState::Conflict { .. }
                    ))
            });
            if let Some(existing) = self.ops.iter_mut().find(|m| {
                m.kind == MutationKind::SyncGlobalEnv && matches!(m.state, MutationState::Queued)
            }) {
                existing.params = mutation.params;
                existing.created_at = mutation.created_at;
                existing.retry_same_params = false;
                existing.attempts = 0;
                let updated = existing.clone();
                save_or_rollback(self)?;
                return Ok((updated, true));
            }
        }
        if mutation.kind == MutationKind::UpdateCard
            && let Some(create) = self.ops.iter_mut().find(|m| {
                m.card_id == mutation.card_id
                    && m.kind == MutationKind::CreateCard
                    && matches!(m.state, MutationState::Queued)
                    && !m.retry_same_params
            })
        {
            if let Some(title) = mutation.params.get("title").and_then(Value::as_str) {
                create.params["title"] = Value::String(title.to_owned());
                if !create.sent_params.is_null() {
                    create.sent_params["title"] = Value::String(title.to_owned());
                }
            }
            if let Some(body) = mutation.params.get("body").and_then(Value::as_str) {
                create.params["body"] = Value::String(body.to_owned());
                if !create.sent_params.is_null() {
                    create.sent_params["body"] = Value::String(body.to_owned());
                }
            }
            let updated = create.clone();
            save_or_rollback(self)?;
            return Ok((updated, true));
        }
        ensure!(self.ops.len() < MAX_OUTBOX_OPS, "Offline queue is full");
        self.ops.push(mutation.clone());
        save_or_rollback(self)?;
        Ok((mutation, false))
    }

    pub fn remove_by_id(&mut self, mutation_id: &str) -> Result<Option<Mutation>> {
        let Some(pos) = self.ops.iter().position(|m| m.id == mutation_id) else {
            return Ok(None);
        };
        let prev_ops = self.ops.clone();
        let removed = self.ops.remove(pos);
        if let Err(err) = self.save() {
            self.ops = prev_ops;
            return Err(err);
        }
        Ok(Some(removed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mk_op(id: &str, card_id: &str, kind: MutationKind, params: Value) -> Mutation {
        Mutation {
            id: id.into(),
            host_id: "remote-1".into(),
            card_id: card_id.into(),
            kind,
            params: params.clone(),
            base: UpdateBase::default(),
            sent_params: params,
            retry_same_params: false,
            state: MutationState::Queued,
            created_at: 100,
            attempts: 0,
        }
    }

    #[test]
    fn persistence_roundtrip_and_inflight_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.outbox.json");
        let mut outbox = Outbox::load(path.clone());
        let mut op = mk_op(
            "m1",
            "c1",
            MutationKind::MoveCard,
            json!({"card_id": "c1", "stage": "develop", "request_key": "m1"}),
        );
        op.state = MutationState::InFlight;
        op.attempts = 1;
        outbox.ops.push(op);
        outbox.save().unwrap();

        let reloaded = Outbox::load(path);
        assert_eq!(reloaded.ops.len(), 1);
        assert_eq!(reloaded.ops[0].state, MutationState::Queued);
        assert!(reloaded.ops[0].retry_same_params);
    }

    #[test]
    fn coalesces_update_into_queued_create_and_respects_dedup_and_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.outbox.json");
        let mut outbox = Outbox::load(path);
        let create = mk_op(
            "m-create",
            "c1",
            MutationKind::CreateCard,
            json!({"id": "c1", "project_id": "p1", "title": "Init", "body": "B0", "request_key": "m-create"}),
        );
        outbox.enqueue(create.clone()).unwrap();
        // Duplicate mutation id does not double-enqueue.
        outbox.enqueue(create).unwrap();
        assert_eq!(outbox.len(), 1);

        let update = mk_op(
            "m-upd",
            "c1",
            MutationKind::UpdateCard,
            json!({"card_id": "c1", "title": "Edited", "body": "B1", "request_key": "m-upd"}),
        );
        let (merged, coalesced) = outbox.enqueue(update).unwrap();
        assert!(coalesced);
        assert_eq!(outbox.len(), 1);
        assert_eq!(merged.params["title"], "Edited");
        assert_eq!(merged.params["body"], "B1");
        assert_eq!(outbox.ops[0].sent_params["title"], "Edited");

        for i in 1..MAX_OUTBOX_OPS {
            outbox
                .enqueue(mk_op(
                    &format!("op-{i}"),
                    &format!("card-{i}"),
                    MutationKind::AddComment,
                    json!({"card_id": format!("card-{i}"), "body": "hi"}),
                ))
                .unwrap();
        }
        let err = outbox
            .enqueue(mk_op(
                "op-overflow",
                "card-overflow",
                MutationKind::AddComment,
                json!({"card_id": "card-overflow", "body": "overflow"}),
            ))
            .unwrap_err();
        assert!(err.to_string().contains("Offline queue is full"));
    }

    #[test]
    fn coalesces_global_env_sync_ops_and_keeps_outbox_pointer_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.outbox.json");
        let mut outbox = Outbox::load(path.clone());
        let op1 = mk_op(
            "g1",
            "",
            MutationKind::SyncGlobalEnv,
            json!({"generation": 1}),
        );
        let (_, coalesced1) = outbox.enqueue(op1).unwrap();
        assert!(!coalesced1);
        let op2 = mk_op(
            "g2",
            "",
            MutationKind::SyncGlobalEnv,
            json!({"generation": 2}),
        );
        let (merged, coalesced2) = outbox.enqueue(op2).unwrap();
        assert!(coalesced2);
        assert_eq!(outbox.len(), 1);
        assert_eq!(merged.params["generation"], 2);
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(disk.contains("sync_global_env"));
    }

    #[test]
    fn reenqueue_global_env_sync_clears_prior_failed_op_and_unblocks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.outbox.json");
        let mut outbox = Outbox::load(path.clone());
        let mut failed_op = mk_op("g-fail", "", MutationKind::SyncGlobalEnv, json!({}));
        failed_op.state = MutationState::Failed {
            message: "token rejected".into(),
        };
        failed_op.attempts = 1;
        outbox.enqueue(failed_op).unwrap();
        assert!(outbox.blocked_cards().contains("__global_env__"));

        let fresh = mk_op(
            "g-retry",
            "",
            MutationKind::SyncGlobalEnv,
            json!({"retry": true}),
        );
        let (queued, coalesced) = outbox.enqueue(fresh).unwrap();
        assert!(!coalesced);
        assert!(!outbox.blocked_cards().contains("__global_env__"));
        assert_eq!(outbox.len(), 1);
        assert_eq!(queued.id, "g-retry");
        assert!(matches!(queued.state, MutationState::Queued));

        let reloaded = Outbox::load(path);
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.ops[0].id, "g-retry");
    }

    #[test]
    fn mid_write_truncate_failpoint_writes_torn_json_and_recovers_via_quarantine_on_reload() {
        use canban_types::FailpointAction;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.outbox.json");
        let fp = Failpoints::shared();
        let mut outbox = Outbox::load_with_durability_and_failpoints(
            path.clone(),
            canban_context::FsDurability::NoSync,
            Some(Arc::clone(&fp)),
        );

        // First persist one valid op so an existing valid file is on disk.
        outbox
            .enqueue(mk_op(
                "m-initial",
                "c1",
                MutationKind::AddComment,
                json!({"card_id": "c1", "body": "ok"}),
            ))
            .unwrap();
        assert_eq!(outbox.len(), 1);

        // Arm crash window #5: next save writes a torn truncated prefix and returns an io::Error.
        fp.arm(
            FP_OUTBOX_MID_WRITE_TRUNCATE,
            FailpointAction::FailOnce("simulated torn write crash".into()),
        );
        let err = outbox
            .enqueue(mk_op(
                "m-crash",
                "c2",
                MutationKind::MoveCard,
                json!({"card_id": "c2", "stage": "develop"}),
            ))
            .unwrap_err();
        assert!(
            err.downcast_ref::<std::io::Error>().is_some(),
            "expected io::Error, got: {err:#}"
        );
        // Failed enqueue rolled back in-memory mutation list cleanly.
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox.ops[0].id, "m-initial");

        // On disk, the outbox file is a non-empty truncated prefix that fails JSON parsing.
        let torn = std::fs::read(&path).unwrap();
        assert!(!torn.is_empty());
        assert!(serde_json::from_slice::<OutboxFile>(&torn).is_err());

        let stats = fp.stats_for(FP_OUTBOX_MID_WRITE_TRUNCATE);
        assert_eq!(stats.armed, 1);
        assert_eq!(stats.fired, 1);

        // Reloading quarantines the corrupt file and recovers cleanly to an empty outbox.
        let mut reloaded = Outbox::load_with_durability_and_failpoints(
            path.clone(),
            canban_context::FsDurability::NoSync,
            Some(Arc::clone(&fp)),
        );
        assert!(reloaded.is_empty());
        let quarantine = Outbox::quarantine_path(&path);
        assert!(
            quarantine.exists(),
            "corrupt outbox should be quarantined at {}",
            quarantine.display()
        );
        assert_eq!(std::fs::read(&quarantine).unwrap(), torn);
        assert!(
            !path.exists(),
            "original corrupt outbox path must be moved aside during quarantine"
        );

        // Subsequent enqueue persists a fresh valid outbox cleanly.
        reloaded
            .enqueue(mk_op(
                "m-after-recovery",
                "c3",
                MutationKind::AddComment,
                json!({"card_id": "c3", "body": "recovered"}),
            ))
            .unwrap();
        let verified = Outbox::load_with_durability(path, canban_context::FsDurability::NoSync);
        assert_eq!(verified.len(), 1);
        assert_eq!(verified.ops[0].id, "m-after-recovery");
    }
}
