# Agent-callable Canban interface

Status: the CLI -> local Unix socket JSON-RPC -> shared AppServer boundary is implemented. See [implementation](../implementation.md) for the exact operation inventory and acceptance evidence. Broader operations below remain design proposals unless listed there. The concrete [MVP interface contract](../interfaces/mvp-contract.md) documents the shared request-bound reporting mechanism and implemented interactive extensions.

## One application authority

Agents should be able to operate Canban directly, as Herdr exposes machine-readable operations. The desktop and agent interface should call the same application service so a move or comment has the same validation, automatic triggers, and timeline history.

```text
Rust / NSH UI --------+
                      |
Agent / CLI ----------+--> Canban application service
                            |
                            +-- Projects / Board / cards
                            +-- comments / resources / timeline
                            +-- workflows / routing / Scheduler
                            +-- database + attachments
                            |
                            +--> herdr-core adapter
                                  terminal execution
```

This Canban API is separate from core's terminal API. Agents should not edit Canban's database files or write to Herdr's layout document to move a card.

## Suggested MVP operations

| Area | Proposed operations |
| --- | --- |
| Discovery | Get API/schema version, available methods, capabilities, and connection identity |
| Projects | List/get/create Projects, inspect directory availability |
| Board/template | Read stages/configuration and validate JSON bindings; no template-customization GUI or complex live migration required |
| Cards | List/filter/get/create, update title/description/tags/resources, move/reorder, archive/restore |
| Timeline/comments | Paginate history, add/revise comments, submit structured mentions |
| Resources | Add references and upload/register durable attachments |
| Execution | Inspect requests and generic Sessions/Runs, submit scoped progress/results/artifacts/check evidence, request cancellation or retry |
| Observation | Subscribe to application changes with reconnection and snapshot recovery |

Workflow/template creation by an agent is part of the product direction. MVP uses JSON-style configuration and ordinary shared configuration validation, with useful incompatible-stage messages. A customization UI and complex active-template migration are deferred. Card text is not itself executable configuration.

A routine card.move into Develop automatically creates work under its binding. No separate agent-launch call is necessary. The agent explicitly reports whether it has completed; a completed report invokes the request callback, with automatic Review/report generation by default. MVP does not independently verify output/checks or wait for terminal exit. The user accepts exceptional residual old execution rather than requiring coordination before progression. Basic callback association and repeated-operation handling remain useful.

Submitted comments with actionable @mentions also create dispatch requests, each starting a fresh conversation when admitted. Entering a stage independently selects an agent and creates another fresh execution. Both use the same Session/Run API and identity; do not expose separate question-session and stage-session resources. A reference to an earlier Run supplies context, not an implicit send/continue target. Canban applies the request-bound result callback and controls execution through core.

## Example move

Illustrative JSON-RPC-style request:

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "method": "card.move",
  "params": {
    "request_key": "move-card-42-to-develop-1",
    "project_id": "project_123",
    "card_id": "card_789",
    "expected_revision": 5,
    "to_stage_id": "develop"
  }
}
```

Illustrative accepted result:

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "result": {
    "operation_id": "operation_321",
    "card_revision": 6,
    "stage_event_id": "event_456",
    "dispatch_request_id": "request_654",
    "dispatch_state": "queued"
  }
}
```

Queued means the service accepted the workflow intent. It does not claim the agent already started. A Backlog move produces a stage event without a stage-entry launch. Unavailable routing or pending startup must be represented in the request/read model instead of pretending a terminal exists.

## Identity and allowed actions

Distinguish human callers, named external agent clients, and the agent assigned to a particular run. The service derives authorship from the authenticated/local connection or run-scoped credential; a JSON author field is not enough.

A run-scoped agent should be able to read its task/context, comment, submit evidence, and report its result. Whether agents can also move stages, archive, change routing, or edit templates depends on declared permissions. The exact permission model is open; use explicit capabilities rather than equating a result report with human acceptance.

Preserve both agent profile and run identity for attribution. A generic profile name such as codex is not sufficient to identify which execution wrote a comment. Do not expose the full raw core API through untrusted preview content.

## Consistency and retries

- Mutations use scoped request keys and return stable operation identity. A repeated accepted command returns the same logical result.
- JSON-RPC message IDs only correlate messages; the request key is the application deduplication identity.
- Expected revisions prevent stale changes. Return structured conflicts with the current revision and a useful hint.
- A move, its timeline entry, and its automatic dispatch intent use one transaction.
- Return observed states: accepted, queued, preparing, active, failed, unknown, or appropriate structured errors.
- An unknown terminal launch or initial-message outcome requires reconciliation; replaying a submitted mention returns its existing request rather than launching another conversation.
- Associate reports with their request/execution and callback. Artifact/check information can enrich context without becoming an independent completion gate. Do not require a full stale-process/resource-handover system for MVP.
- History and list responses need bounded pagination. Resource upload limits and content handling should be explicit in the eventual schema.

Recommend a discoverable versioned schema and a thin CLI over the same operations, borrowing Herdr's interface pattern. The concrete current proposal is local JSON-RPC over a Unix socket for macOS MVP, as specified in the interface contract. The proposal has not been implemented. Full subscription/delivery design remains deferred; a durable timeline reader is distinct from a live notification stream.

## Completion, blockage, and events

Use execution.report through the proposed Canban CLI so existing agents launched through Herdr can declare working, completed, blocked, or failed. The CLI reads the request/attempt association injected at launch; message/Markdown body are optional. See the [payload, receipt, and retry contract](../interfaces/mvp-contract.md#agent-report-rpc). A completed declaration directly drives the stored callback without a provider-specific evaluator or independent quality gate.

Blockage marks the card needs attention and generates an internal notification event. API and UI activity participate in the Rust enum/in-memory channel/topic-subscription direction. The internal bus design and macOS notification adapter are deferred beyond the minimum MVP implementation; see [event direction](events-and-notifications.md).

## Acceptance

A fixture agent should create a card, add a comment/resource, move it to Develop, observe the resulting request/run, submit a scoped result, and read the unified timeline using only the interface. Replaying any mutation must preserve card numbers and trigger counts. JSON configuration must report incompatible stage mappings; no visual editor/live-migration feature is required. API and UI operations must produce equivalent application outcomes.
