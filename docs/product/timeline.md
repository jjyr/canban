# Unified card timeline and event format

Status: history requirements reconciled 2026-11-11. The accepted next [event/subscription contract](events-and-notifications.md) owns the shared envelope, catalog and delivery; [workflow rules](workflow-engine.md) own run/step progress. See [implementation coverage](workflow-engine.md) for the delivered slice and verification. Timeline is the readable Card history projection, not a second event authority. Payload details below remain proposals unless covered by [implementation](../implementation.md).

## One history surface

A card presents comments, workflow changes, execution history, and relevant code activity in one ordered timeline. Comments can come from users or agents. Do not create a separate chat history that becomes the only location for an agent's explanations or the user's feedback.

```text
Project P / Card #42
  seq 1   User       created card
  seq 2   User       edited description
  seq 3   User       pasted image / linked resource
  seq 4   User       moved Backlog -> Develop
  seq 5   Scheduler   selected default agent; queued development
  seq 6   Workflow   prepared worktree
  seq 7   Agent      started run R1 in terminal t7
  seq 8   Agent      commented with a question
  seq 9   User       commented and mentioned @codex
  seq 10  Scheduler   delivered/queued the Question for its Card/profile conversation
  seq 11  Agent      replied with explicit Question/request binding
  seq 12  Agent R1   declared development completed; optional result context
  seq 13  Workflow   automatically moved Develop -> Review
  seq 14  Scheduler   queued a fresh execution linked to that result/artifact
  ...     Workflow   review activity
  ...     Human      recorded disposition
  ...     Lifecycle archived card
```

The default Develop -> Review transition follows explicit completion. In the unified design, the owning step progresses; direct questions retain independent completion semantics. Mentions deliver to or start/resume the Card/profile conversation under the [Session contract](sessions.md). WorkflowRun is distinct from Conversation/Request/Attempt. Planning/approval can appear here while the Card remains in Backlog. Worktrees remain through Archive under the current lifecycle contract.

## Current native presentation

The Card reader connects human/Agent narrative cards and semantic milestone
badges along one continuous vertical spine, collapsing runs of two or more
consecutive milestones onto their latest row with an inline `+N earlier`
disclosure pill. Routine metadata mutations (`card.created`, `card.updated`,
column bookkeeping, transient merge-start notices, heartbeat disconnects and
resolved runtime input prompts) stay stored for triggers/audit while omitted
from the Activity projection unless explicitly deep-linked. Long Markdown folds
by visual height, without truncating its source. Execution details expand at
the corresponding request in the history rather than preceding the entire
conversation. Ordering, unread positions and exact request identities remain
unchanged. See [Card conversation](../card-conversation.md) for the implemented
rules and local-only quick reply scope.

## Shared facts and Card history

Use the [event envelope and catalog](events-and-notifications.md#event-envelope) rather than a second Timeline schema. Stable IDs, Card sequence, journal offset, provenance, operation/causation and workflow/step/request/attempt references survive grouping/pagination. Timestamps do not establish processing order.

Local events retain revision/artifact references; workflow steps and decisions retain exact run/version associations. A move is one canonical fact with entry/exit subscription views, not three separately authored history entries.

GitHub activity can appear beside local history from disposable cache and durable event metadata. Do not permanently copy remote prose/diffs/snapshots into Timeline. Source facts may have no Card; later association does not rewrite/replay history. Missing bodies and observation gaps are explicit. See the [GitHub contract](../github-workspace.md).

Raw terminal bytes, keystrokes, drafts, hover and focus remain separate streams/transient state. Project/template changes have project-level facts; only affected Cards receive corresponding changes.

## Comments and mentions

A comment is a first-class timeline entry with a stable comment_id, revision, author, rich body, resource references, and structured mentions. User and agent comments use the same rendering and revision mechanism. An agent can also submit structured result data; that is not reduced to an unparseable comment.

Proposed comment payload:

```json
{
  "comment_id": "comment_12",
  "revision": 1,
  "body": {
    "format": "markdown",
    "text": "@codex Please explain why this test is needed."
  },
  "mentions": [
    {
      "mention_id": "mention_34",
      "profile_id": "codex",
      "context_run_id": "run_56"
    }
  ],
  "resource_ids": []
}
```

A structured mention identifies the Profile; text alone is not executable. A previous execution reference is context, not permission to complete its assignment. Submission durably records the comment and Question/request association. Live questions are delivered through the existing invocation; absent/exited conversations start/resume through Scheduler. Replay returns the existing intent without blindly resending uncertain input.

No question/stage Session classes are needed. WorkflowRun adds process-level progress above Conversations/Requests/Attempts. See [Session model](sessions.md).

Recommendation: comment edits preserve revisions without automatically redelivering old mentions. New mentions/resends are attributable separately. Removal retains a tombstone; retention/redaction needs separate policy. Ordinary replies cannot approve a workflow decision. (Implemented for user-authored local comments via `comment.update` / `comment.updated` and `comment.delete` / `comment.deleted`, and for user-authored remote GitHub comments/reviews via `github.remote.comment.update` and `github.remote.comment.delete`.)

## Resources and pasted images

Use an extensible resource model with typed references: notes, cards, files, URLs, images, attachments, changes, checks, and future kinds. A resource should identify what was referenced at the time; its live target and an optional captured revision/snapshot are separate.

For pasted images, persist bytes in Canban-managed attachment storage and record an attachment ID, media type, size, and optional digest/alt text. Store a reference in the card body/comment. Do not rely on a temporary clipboard path or embed every image directly in every event.

The [data architecture](../data-architecture.md) uses the user-selected ~/.canban/ home and proposes file-backed execution context/report bodies. Timeline entries store structured outcomes, concise summaries, and artifact references; full report bodies are loaded on demand. Ordinary comments and editable content revisions can remain database-backed.

A failed attachment save remains visibly pending/failed. Commit a content event only when its referenced attachment is durably available. Partial saves need orphan cleanup that never deletes referenced attachments. Limits, export, deletion, and backup behavior remain product/implementation decisions.

Arbitrary resource references do not imply running arbitrary referenced HTML/scripts inside a privileged UI bridge.

## Transaction, concurrency, and replay

Recommended contracts:

- Card mutation, projection revision and canonical fact commit together. Durable dispatch may consume the journal later; live channel delivery is not required.
- Persist delivery/entry intent before advancing its dispatch cursor; persist a consumer effect/checkpoint/next intent before acknowledgement. See [delivery transactions](events-and-notifications.md#transactions-and-delivery).
- Record Request/Attempt intent before external IO. Later observations append facts.
- Results retain run/step/request/attempt binding. Engine commits transitions and resulting intents consistently. Repeated reports cannot duplicate progression; workflow moves are not attributed to a human drag.
- A retried mutation with the same scoped request key returns its original operation/result; it does not allocate another card number or repeat a stage trigger.
- Use expected card/configuration revisions to reject stale competing changes. Deduplication must be checked before rejecting a retry solely because its first attempt already changed the revision.
- Allocate sequence monotonically per card. Event IDs and causal links survive display grouping and pagination.
- Runtime source-event deduplication is scoped to the connection/core epoch. Core sequence numbers are not permanent Canban event IDs.
- Retain basic request/callback association. The user's MVP deliberately excludes elaborate old-process, writer-handover, or generation-coordination systems; this history draft must not reintroduce them as release gates.
- Show reconciliation gaps explicitly. A fresh snapshot does not reconstruct every missed event.

The accepted journal/current-state design supports durable reactions without requiring a complete event-sourcing framework. Corrections use new events and revision references rather than silently rewriting history.

## Readability and retention

The default timeline should support reading the whole story and filtering comments, execution, changes, or system events without moving them into disconnected data silos. Long output can collapse into expandable run summaries. Selecting a run should reveal exact captured context, profile/workflow, terminal, evidence, and callbacks.

Archive retains the timeline and project-local number and is the boundary for managed-worktree handling. Worktrees remain through development, Review, and rework. Core's bounded terminal retention cannot be Canban's history retention policy. Export, large-artifact retention, and any permanent deletion policy must be decided explicitly.

The user selected zstd/lz4 compression of archived Card data, including related Agent execution data and results. Detailed Timeline/content can move into the compressed package while SQLite retains Card lookup records and archive references. History remains readable on demand, with stable identities. The [data architecture](../data-architecture.md#compressed-card-archives) proposes packaging, safe publication, shared-resource handling, and restoration; archive compression is not history deletion or a full Herdr runtime backup.
