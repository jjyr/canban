# Unified card timeline and event format

Status: unified comments/events and per-execution history are confirmed product requirements. The envelope/catalog below is an earlier history-format draft, not a finalized Rust event enum, topic schema, delivery protocol, or MVP requirement to implement the whole catalog. The user chose a Rust enum + in-memory channel + topic subscribers and deferred detailed event architecture until after MVP; see [event direction](events-and-notifications.md).

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
  seq 10  Scheduler   queued a fresh execution for the mentioned question
  seq 11  Agent R2   answered in a new conversation linked to that comment
  seq 12  Agent R1   declared development completed; optional result context
  seq 13  Workflow   automatically moved Develop -> Review
  seq 14  Scheduler   queued a fresh execution linked to that result/artifact
  ...     Workflow   review activity
  ...     Human      recorded disposition
  ...     Lifecycle archived card
```

The default automatic Develop -> Review transition is confirmed and configurable by template, driven by the agent's completion declaration. Submitted @mentions and executable-stage entries both schedule fresh conversations using one Session/Run model. Their callbacks differ. Worktrees are retained until Archive; detailed cleanup treatment can be refined during implementation.

## Proposed common envelope

Every persisted card event has one envelope. A proposed JSON example:

```json
{
  "schema_version": 1,
  "event_id": "event_456",
  "project_id": "project_123",
  "card_id": "card_789",
  "card_number": 42,
  "sequence": 18,
  "type": "card.stage_changed",
  "recorded_at": "2026-11-05T09:18:00Z",
  "actor": {
    "kind": "user",
    "id": "user_local"
  },
  "operation_id": "operation_321",
  "causation_id": null,
  "run_id": null,
  "workflow_revision": null,
  "data": {
    "from_stage_id": "backlog",
    "to_stage_id": "develop",
    "stage_entry_id": "entry_654",
    "reason": "user_move"
  }
}
```

| Field | Proposed contract |
| --- | --- |
| schema_version / type | Version the common envelope and typed payload registry; preserve unknown fields/types for later readers |
| event_id | Stable immutable event identity |
| project_id / card_id | Canonical association; card_number is a display/reference convenience |
| sequence | Strictly increasing, transactionally allocated order within the card; never derived from wall-clock timestamps |
| recorded_at | Service-assigned time when the observation or action was recorded |
| actor | Validated user, agent, or system identity; distinguish a service action from an agent assertion |
| operation_id | Groups events from one accepted command or workflow operation |
| causation_id | Links the action to its trigger, such as stage entry -> request -> run |
| run_id | Optional explicit execution association |
| workflow_revision | Optional captured workflow version; full request/configuration data may be referenced separately |
| data | Typed payload or references to immutable revisions/artifacts |

Source observations can additionally carry source_event_id and source_occurred_at. These do not control timeline ordering or become trusted human authorship. A reconnect may record an observation later than it actually occurred; preserve that distinction.

The service derives actor identity from the UI/agent connection and run binding. A client-provided display name or terminal label cannot impersonate a person. Imported remote authors, if a backend is added later, retain source provenance.

## Event families

Names below are proposals. Finalize payload schemas before implementation rather than scattering free-form event strings across modules.

| Family | Example events | Required association/evidence |
| --- | --- | --- |
| Card content | card.created, card.description_changed, card.title_changed | Before/after or immutable content/configuration revisions |
| Workflow | card.stage_changed, card.order_changed, card.template_mapped | Stable stage IDs, entry identity, template/migration operation |
| Comments | comment.created, comment.revised, comment.removed | Stable comment ID and revision; original event remains |
| Resources | resource.added, resource.removed | Typed resource ID, durable attachment reference or URI |
| Routing/admission | dispatch.queued, dispatch.routing_blocked, dispatch.paused, dispatch.canceled | Trigger, workflow, routing decision/reason, captured context |
| Preparation | workflow.preparation_started, workflow.preparation_finished, workflow.preparation_failed | Request/run, callback identity, checkout/resource or error |
| Execution | run.started, run.status_observed, run.input_needed, run.cancel_requested, run.finished | Request/attempt binding; completion follows the agent's explicit report without a process-exit gate |
| Attention/notification | needs-attention and internal-notification event names are not yet fixed | Blocked card, originating request/report, and notification context; native delivery later |
| Recovery | run.launch_unknown, run.reconciled, observation.gap | Connection epoch, known evidence, unresolved facts |
| Mention scheduling | mention.requested, mention.answered | Comment/mention ID, addressed profile, dispatch request and fresh Run; launch ambiguity uses the normal Run events |
| Code/checks | change.recorded, commit.observed, check.recorded | Repository + exact change/commit identity, check source/result |
| Review/outcome | review.reported, outcome.recorded | Reviewed artifact identity and author; distinguish report from approval |
| Lifecycle | card.archived, card.restored | Actor/reason and recorded outcome, without deleting prior history |

Project creation and Board/template changes also need project-level history. When a migration changes individual cards, emit corresponding card events linked to the project-level operation. Do not fabricate a card event for every unaffected card or claim a full GitHub-style push feed before source observation exists.

Raw terminal bytes, keystrokes, typing drafts, hover, and focus are not all separate card events. Persist meaningful execution/state/content changes; provide terminal output as a distinct bounded stream or referenced artifact. Group events visually when useful while retaining their identities and details.

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

A structured mention identifies the profile to schedule; the text alone is not executable. context_run_id is optional context for the new execution, not a terminal/Run to reuse. Submitting a comment records the comment and its new dispatch intent consistently. When admitted, that request starts a fresh conversation under a new Run. Replaying the accepted submission returns the existing intent rather than another conversation.

No session_type field is needed: request causality links the result to its initiating comment or stage entry. All Runs share the same envelope, lifecycle, and observation mechanism. See [Session model](sessions.md).

Recommendation: editing a comment preserves old revisions and does not automatically redeliver its old mentions. A newly added mention or explicit resend is a new, attributable action. Removing a comment preserves an event/tombstone; content retention/redaction rules require a separate policy. Neither an idle nor an active matching profile changes the fresh-conversation rule. A future explicit continuation operation remains a separate open feature.

## Resources and pasted images

Use an extensible resource model with typed references: notes, cards, files, URLs, images, attachments, changes, checks, and future kinds. A resource should identify what was referenced at the time; its live target and an optional captured revision/snapshot are separate.

For pasted images, persist bytes in Canban-managed attachment storage and record an attachment ID, media type, size, and optional digest/alt text. Store a reference in the card body/comment. Do not rely on a temporary clipboard path or embed every image directly in every event.

The [data architecture](../data-architecture.md) uses the user-selected ~/.canban/ home and proposes file-backed execution context/report bodies. Timeline entries store structured outcomes, concise summaries, and artifact references; full report bodies are loaded on demand. Ordinary comments and editable content revisions can remain database-backed.

A failed attachment save remains visibly pending/failed. Commit a content event only when its referenced attachment is durably available. Partial saves need orphan cleanup that never deletes referenced attachments. Limits, export, deletion, and backup behavior remain product/implementation decisions.

Arbitrary resource references do not imply running arbitrary referenced HTML/scripts inside a privileged UI bridge.

## Transaction, concurrency, and replay

Recommended contracts:

- A card mutation, projection revision, and its event(s) commit together. Stage entry and its automatic dispatch intent also commit together.
- Record request/attempt identity before external IO. A later runtime observation appends another event.
- On the current development result, record result linkage, the configured automatic Review transition, and next dispatch intent consistently; the stage event is caused by that result, not attributed to a human drag. A repeated result submission cannot create another transition/request.
- A retried mutation with the same scoped request key returns its original operation/result; it does not allocate another card number or repeat a stage trigger.
- Use expected card/configuration revisions to reject stale competing changes. Deduplication must be checked before rejecting a retry solely because its first attempt already changed the revision.
- Allocate sequence monotonically per card. Event IDs and causal links survive display grouping and pagination.
- Runtime source-event deduplication is scoped to the connection/core epoch. Core sequence numbers are not permanent Canban event IDs.
- Retain basic request/callback association. The user's MVP deliberately excludes elaborate old-process, writer-handover, or generation-coordination systems; this history draft must not reintroduce them as release gates.
- Show reconciliation gaps explicitly. A fresh snapshot does not reconstruct every missed event.

An append-only history with transactional current-state tables is a proposed storage strategy, not a commitment to implementing a complete event-sourcing framework. Corrections use new events and revision references rather than silently rewriting history.

## Readability and retention

The default timeline should support reading the whole story and filtering comments, execution, changes, or system events without moving them into disconnected data silos. Long output can collapse into expandable run summaries. Selecting a run should reveal exact captured context, profile/workflow, terminal, evidence, and callbacks.

Archive retains the timeline and project-local number and is the boundary for managed-worktree handling. Worktrees remain through development, Review, and rework. Core's bounded terminal retention cannot be Canban's history retention policy. Export, large-artifact retention, and any permanent deletion policy must be decided explicitly.

The user selected zstd/lz4 compression of archived Card data, including related Agent execution data and results. Detailed Timeline/content can move into the compressed package while SQLite retains Card lookup records and archive references. History remains readable on demand, with stable identities. The [data architecture](../data-architecture.md#compressed-card-archives) proposes packaging, safe publication, shared-resource handling, and restoration; archive compression is not history deletion or a full Herdr runtime backup.
