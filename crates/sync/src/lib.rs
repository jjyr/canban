//! Offline mutation outbox, optimistic snapshot projection, field-level
//! conflict merging, replay engine, and bounded read cache shared across
//! remote-host clients.

pub mod merge;
pub mod outbox;
pub mod project;
pub mod reads;
pub mod replay;

pub use merge::{FieldMergeOutcome, classify_card_update, conflict_details};
pub use outbox::{MAX_OUTBOX_OPS, Mutation, MutationKind, MutationState, Outbox, UpdateBase};
pub use project::{
    apply, compute_pending_sync, offline_resolve_preview, pending_comment_events, synthetic_receipt,
};
pub use reads::{DEFAULT_READ_CACHE_BUDGET_BYTES, ReadCache};
pub use replay::{DrainReport, Transport, drain};

use serde_json::Value;

/// Classification for an outgoing remote RPC when offline or encountering a connection failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueDecision {
    Queue(MutationKind),
    RejectOffline(&'static str),
    NotQueueable,
}

/// Classify whether `method` with `params` can be queued for offline replay.
pub fn classify_remote_mutation(method: &str, params: &Value) -> QueueDecision {
    match method {
        "card.create" => QueueDecision::Queue(MutationKind::CreateCard),
        "card.update" => {
            let Some(obj) = params.as_object() else {
                return QueueDecision::RejectOffline("Requires a connection to the remote host");
            };
            let allowed = obj.keys().all(|k| {
                matches!(
                    k.as_str(),
                    "card_id"
                        | "revision"
                        | "title"
                        | "body"
                        | "view"
                        | "fields"
                        | "request_key"
                        | "host_id"
                )
            });
            if allowed {
                QueueDecision::Queue(MutationKind::UpdateCard)
            } else {
                QueueDecision::RejectOffline("Requires a connection to the remote host")
            }
        }
        "card.move" => QueueDecision::Queue(MutationKind::MoveCard),
        "comment.add" => QueueDecision::Queue(MutationKind::AddComment),
        "card.resolve" => QueueDecision::Queue(MutationKind::ResolveCard),
        _ => QueueDecision::NotQueueable,
    }
}

/// Whether `method` responses are stored in and served from the per-host bounded read cache.
pub fn is_read_cacheable_method(method: &str) -> bool {
    matches!(
        method,
        "timeline.list"
            | "git.compare"
            | "git.refs"
            | "git.log"
            | "git.commit_detail"
            | "review.preview"
            | "git.file_lines"
    )
}
