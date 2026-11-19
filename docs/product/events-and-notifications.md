# Events, subscriptions and notifications

Status: user-confirmed direction and initial implementation, 2026-11-11. The [implementation contract](workflow-engine.md) records concrete wire compatibility, operations, polling coverage and evidence. Durable journal offsets, subscription delivery and the unified workflow dispatcher now exist; channels remain wake-up/invalidation transport. This document defines the broader semantics, with unimplemented extensions identified.

## Facts, subscriptions and execution

An event records a committed fact. A subscription selects facts and targets an internal consumer or workflow entry. Workflow Engine interprets configured actions; Scheduler admits Agent execution. An event alone does not authorize Agent launch, human approval or GitHub publication.

```text
UI / CLI operations     GitHub observations      Agent reports
          +--------------------+---------------------+
                               |
                  validate and commit product facts
                               |
                    durable event journal (SQLite)
                               |
                    in-memory wake-up notification
                               |
                       subscription dispatcher
                               |
                    durable delivery / entry intent
                               |
              +----------------+----------------+
              |                |                |
          Timeline        Notifications     Workflow Engine
                                                |
                                  Card action / wait / Agent request
                                                             |
                                                         Scheduler
```

State tables remain authoritative. The journal supports history/reactions, not a requirement to rebuild the application exclusively from events. Keep one canonical fact rather than independently writing Timeline and automation logs. The existing event store may evolve for this purpose; physical layout is not selected. Transient UI invalidation/runtime streams need not all enter the durable journal.

## Event envelope

This is the logical envelope, not the current serialized `Event` struct or a final JSON schema.

| Field | Contract |
| --- | --- |
| ID / type / schema version | Stable immutable identity and versioned typed payload |
| Journal offset | Transactional consumption order within one AppServer authority/home; not wall-clock order |
| Subject | Card, GitHub Issue/PR, workflow run, execution or another supported object |
| Project / Card ID | Optional for unbound repository events; stable IDs, not display numbers |
| Card sequence | Monotonic local Card history order; journal offset also orders associated source facts |
| Related Cards | Recorded associations with source/reference/delivery relationship kind |
| Actor / origin | Validated local caller or attributed remote author, plus local/GitHub/workflow provenance |
| Occurred / recorded time | Known source occurrence time distinct from service observation/commit time |
| Operation / causation | Accepted command, triggering event and causal chain |
| Workflow / step / request / attempt | Applicable execution identities; workflow run is not provider Session |
| Payload | Typed facts, changed field paths, revision identities and artifact references |
| Source evidence | Remote event/delivery ID, object revision/fingerprint, observation mode and gap evidence when available |

Use Rust enums/shared types internally and stable serialized names externally. Readers preserve unknown data; consumers cannot execute unknown types as if understood. Server-derived actor identity matters: a GitHub author or client-supplied name is not proof of local human approval.

## Local catalog

| Family | Required semantics |
| --- | --- |
| Membership | `card.created`, `card.restored`, `card.removed`, `card.archived`; distinguish manual creation, automatic intake and restoration |
| Content | `card.content_changed`; changed field paths and relevant before/after revisions; local instructions differ from remote source prose |
| Column | Existing `card.stage_changed` canonical fact with from/to, transition/entry identities, reason and workflow causality |
| Comments/replies | `comment.created`, `comment.replied`; stable identity, reply target and request association; revisions/tombstones for changes/removal |
| Resources | Added/removed references and durable attachment identity |
| Execution | Queued/started/reported/failed/uncertain facts with Request/Attempt binding; progress differs from completion |
| Workflow/approval | Run/step state and approval requested/resolved with exact result/version |
| Attention/notifications | Input/decision state and delivery/read state; reading does not resolve a decision |

Finalize new enum variants in shared types; existing serialized names need explicit compatibility handling rather than silent reinterpretation.

### Column entry and exit

Expose `card.column_entered` / `card.column_left` as subscription views over canonical membership/transition facts; do not persist three competing accounts of a move.

```text
card.stage_changed(T1, backlog -> develop)
                 |
                 +-- column_left(backlog, previous entry)
                 +-- column_entered(develop, new entry)
```

Same-column reorder is neither leave nor enter. Creation/restoration enters its column; archive/removal leaves active Board membership. Preserve reasons. Restore does not implicitly schedule work: default entry bindings exclude restoration unless configured otherwise.

For `card.stage_changed`, entry selectors match only `to` and exit selectors match only `from`, including replay of historical events whose `stage` metadata contains the old column. Creation/restoration and archive/removal use `stage` for their respective membership direction. Each type alternative applies its own column constraint. New transition facts are appended after saving the Card, so `stage` and `card_revision` describe the resulting Card in the same transaction.

A view retains the original event ID and stable match identity. One subscription matches an original fact once even if multiple selector alternatives match; independent subscriptions each receive it. Workflow-owned moves use the same facts with ownership preventing duplicate starts.

## GitHub source events

Target Issue events: discovery/open/edit/close/reopen, comments, labels and assignees. PRs additionally cover head/draft changes, merge, review requests/submissions/dismissals, threads and checks. This semantic scope does not claim today's adapter captures every event.

```text
GitHub fact (possibly no Card)
       +-- repository / remote-object subscriber (`selector.repository`)
       +-- configured intake -> Card created with source binding
       +-- source-Card subscriber when associated
```

- **Canonical naming and actor identity:** PR and Issue observations use a single canonical event taxonomy (such as `github.issue.opened`, `github.issue.comment_created`, `github.pr.comment_created`, `github.pr.review_submitted`, `github.pr.head_changed`) rather than emitting competing PR-Card-only aliases that would double-fire wildcard `github.pr.*` subscriptions. The canonical `actor` field is uniformly formatted as `github:<login>` (derived from the GitHub user login rather than numeric user IDs).
- **Body-free lightweight payloads:** Durable GitHub event payloads carry lightweight typed metadata (repository, object kind/number, event IDs, actor login, `self_action`, changed-field/revision evidence, and related-Card associations) without embedding full Issue/PR/comment prose bodies. Downstream workflow scripts fetch full bodies or diffs on demand (for example via `gh api` or `github.context`), preserving the [disposable-cache contract](../github-workspace.md).
- **Repository-scoped cardless matching:** Unbound repository events keep `project_id = ""` and `card_id = ""` so one canonical remote fact is recorded even when multiple Projects share a repository. Project workflow rules targeting `github.*` bind `selector.repository` from `project.github.repository`, allowing `canban_workflow_engine::matches` to match cardless repository events without stamping `event.project_id` or requiring `data.related_cards`.
- **Stable identities and provenance:** Use stable host/repository/object IDs, not only mutable names/numbers. Source, reference and delivery relationships differ. Per-Card subscriptions include local/source facts by default; reference/delivery activity is explicit. Associations used for routing remain attributable; later linking creates an association fact and does not rewrite/replay old events.

Remote events retain provenance. They never call local `comment.add` or interpret remote `@mentions` as local commands. An enabled rule can react separately. Remote close/merge does not move/archive a Card without a configured action.

### Discovery, polling and gaps

Current background/source refresh is roughly 120 seconds without webhook. Polling sees observations/final differences, not every intermediate edit, close/reopen or deleted comment. A broad `updated_at` change is not evidence for every specific field change.

- **Repository-level polling independent of `intake: off`:** When a connected Project has active `github.*` subscriptions (including cardless rules), background polling scans repository-level endpoints—`GET /repos/{owner}/{repo}/issues?since=...` (filtering out pull requests via `pull_request` presence) and `GET /repos/{owner}/{repo}/issues/comments?since=...`—even when Issue/PR Card `intake` is set to `off`.
- **Durable baseline watermarks (`opened` vs. `discovered`):** First observation is `discovered`, not necessarily `opened`. Each polled repository maintains a durable baseline watermark in `source_observations` (never `project.github.synced_at` or disposable cache files). A newly seen object emits `opened` only when `created_at >= baseline`; older pre-existing objects observed for the first time (for example after pagination cutoffs or enabling a rule) emit `discovered` instead of falsely firing `opened`.
- Emit Card creation after completing its source binding in the same transaction, so filters see the final source kind.
- Scan required pages/collections with bounded work and durable progress. Failed/partial scans cannot become successful empty baselines or advance watermarks past unprocessed observations.
- Keep minimal source deduplication/progress/revision metadata outside disposable response caches. Clear/eviction/restart cannot make known objects/comments new. Missing evidence produces a gap/reconciliation, not invented history.
- Compare fields/revisions only with adequate evidence. A missing partial-page item is not deletion. Late older fetches cannot replace newer observations.
- A future webhook adapter improves latency/completeness but still requires deduplication and reconciliation/polling recovery. Transport does not change event semantics.

Preserve the [GitHub cache contract](../github-workspace.md): durable event IDs, changed-field/revision evidence and receipts do not imply permanent remote title/body/comment/diff snapshots. This adds event metadata, not the removed remote-content mirrors. Local plans/reports and accepted publication receipts remain durable. Exact metadata retention remains implementation work.

## Subscription contract

| Setting | Meaning |
| --- | --- |
| Scope | All authorized events, Project, Card, repository (`selector.repository`) or remote object |
| Types | Explicit types, families or all; validated against the registry |
| Filter | Column, changed fields, source kind, actor/origin, `exclude_self`, membership reason and supported typed properties |
| Target | Stable internal consumer identity or workflow definition/entry (Card-bound or cardless `WorkflowRun`) |
| Revision / activation | Captured rule revision and atomic activation offset; future events by default |
| Mode | Durable for actions/history processing; ephemeral for replaceable UI invalidation |
| Policy | Explicit pause, failure/retry and optional edit-coalescing behavior |

All-event scopes remain within allowed access. Card scopes use relationships, not payload text search. Cardless Project rules targeting GitHub events match via `selector.repository` without requiring `event.project_id` or `data.related_cards`. Setting `exclude_self: true` filters out events marked `self_action: true` (authored by the active GitHub identity or matching a Canban-originated remote write receipt). Reply filters can address a comment/request; content rules match fields, not every unrelated revision.

Rule edits record a new revision/offset boundary: dispatch lag must not apply today's rule retroactively. Created deliveries retain their rule revision. The initial implementation disables consumption while a rule is off; saving/enabling a revision starts at a new high-water mark and skips that interval. Already created deliveries retain their revision. An enabled rule with undispatched backlog must drain before replacement. More pause/backfill controls remain future design.

Historical match preview is read-only. Action replay is an explicit run/replay operation with its own identity/deduplication scope, not resetting a production cursor. New Issue automation defaults to future intake; existing Card processing is separate.

## Transactions and delivery

```text
Producer transaction
  update authoritative state + append canonical fact
  commit
      |
      +-- wake dispatcher (advisory)

Dispatcher transaction
  match event + applicable subscription revision
  persist unique delivery / workflow entry intent
  advance dispatch cursor
  commit
      |
      +-- consume durable intent

Consumer transaction
  apply local effect / checkpoint / persist execution intent
  acknowledge delivery
  commit
```

Dropped channel signals cannot lose committed work; startup and bounded recovery scans inspect pending progress. Deliveries are unique by subscription/original event; deliberate replay has separate identity. Steps/commands also have idempotency keys. Cursor/ack advancement is atomic with durable intent/effect at the applicable boundary. A failing subscriber cannot roll back an accepted producer command or block unrelated consumers.

Use at-least-once delivery and idempotent consumers, not an end-to-end exactly-once promise. Retry safe known failures with bounded backoff and visible error/exhaustion. Persist intent before external IO. For cardless event-triggered workflows, the dispatcher transaction atomically persists a queued cardless `WorkflowRun` (`card_id: None`, `trigger_event_id`) and links `EventDelivery.workflow_run_id` before waking the async workflow runner outside the SQLite write transaction—keeping cycle tracking (`workflow_ancestor`) intact and avoiding blocking IO inside database transactions. Unknown Agent launches, GitHub publication and runtime controls require reconciliation, not blind replay. Existing operation receipts remain authoritative.

Offsets order local recording, not remote occurrence. Preserve Card order where processing requires it; later transitions cannot bypass an earlier pending decision. Unrelated consumers need no global completion order.

Retention must protect durable cursors, pending deliveries and checkpoints. Cache cleanup/Card archival cannot discard unprocessed facts. If an explicit future retention policy makes a consumer fall behind available history, show a gap and reconcile instead of silently skipping/replaying effects. Retention limits are not selected here.

## Loops, history, notifications and timers

Keep operation/run/step causality. A workflow-owned column move continues its run instead of restarting it. Exclude self-generated activity where appropriate, validate automatic cycles and impose execution/trigger limits. Actor filters alone cannot prevent cross-workflow loops.

For **cross-GitHub loop suppression**, remote writes re-enter through polling with `origin: "github"` and no `workflow_run_id`, so local `workflow_ancestor` checks alone cannot break a remote feedback loop, and matching only Canban's logged-in account can miss writes performed under a different token (such as an inherited shell `GH_TOKEN` or bot PAT). Canban therefore combines `selector.exclude_self` with **receipt-tracked remote object IDs** (`comment_id` and `review_id` recorded in `source_observations` whenever Canban publishes a comment or review) so subsequent polls recognize Canban-originated writes and suppress re-triggering.

Optional edit coalescing retains event/checkpoint coverage. It cannot silently merge distinct comments, replies, approvals or lifecycle events. [Workflow rules](workflow-engine.md) own overlap, rework and stale results.

Timeline groups relevant canonical local facts and workflow/Agent artifacts. Source activity can appear beside it from cache/metadata without permanent remote prose copies. Raw terminal bytes, drafts, keystrokes and hover remain separate streams/transient state.

Blocked/input-needed activity projects to attention/internal notifications. Opening/reading a Card updates unread state, not approval or request resolution. Native macOS notifications remain deferred independently of subscriptions.

Kanban owns Card timing intent; Scheduler owns readiness/clock waiting; Workflow Engine applies timed actions. Adopting events does not select advanced cron/recurrence or missed-run policies.

## Owners and future checks

Shared types own event/selector/action contracts; domains produce facts; Storage persists journals/progress; AppServer wires dispatch/lifecycle; Workflow Engine handles entries; Scheduler admits execution. No separate broker service/thread per subscriber is required.

Future acceptance covers Card/reply/content/column selectors, all-event versus specific-Card scope, relationship kinds, duplicate remote delivery/cache cleanup, first-sync baselines, partial scans/gaps, source-bound creation, dispatcher/consumer crash boundaries, subscriber isolation, activation revisions, loops and unknown external effects. These are requirements, not newly executed checks.
