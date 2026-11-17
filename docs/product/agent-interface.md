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
| Cards | List/filter/get/create, update title/description/resources, move/reorder, archive/restore |
| Timeline/comments | Paginate history, add/revise comments, submit structured mentions |
| Resources | Add references and upload/register durable attachments |
| Execution | Inspect requests and generic Sessions/Runs, submit scoped progress/results/artifacts/check evidence, request cancellation or retry |
| Observation | Subscribe to application changes with reconnection and snapshot recovery |

Initial implementation (2026-11-11; see [concrete API and limits](workflow-engine.md)): Agents author/modify the same declarative [workflow definitions](workflow-engine.md) that users manually edit. Both paths share validation, expected revisions and published definition identity. A generic viewer displays supported definitions/runs; a graph editor is not required. Schema discovery, definition read/validate/save, subscription configuration and run/decision inspection need shared operations; actual operations are listed in the implementation contract. Saving a definition is distinct from enabling a trigger, starting/replaying a run or approving an existing gate. Card prose is not executable configuration.

Manual/subscribed/column entries use one engine and the existing execution queue. Convert current stage configuration/callbacks instead of retaining another executor. Active runs capture revisions; authoring must not rewrite their assignments or pending decisions. Settings configuration conversations and onboarding defaults are the next UX/ownership discussion, not yet implemented.

A routine card.move into Develop automatically creates work under its binding. No separate agent-launch call is necessary. The agent explicitly reports whether it has completed; a completed report invokes the request callback, with automatic Review/report generation by default. MVP does not independently verify output/checks or wait for terminal exit. The user accepts exceptional residual old execution rather than requiring coordination before progression. Basic callback association and repeated-operation handling remain useful.

Submitted @profile comments use the existing Card/profile conversation: deliver a Question when live or start/resume through Scheduler when absent/exited. Workflow Agent steps retain independent Request/Attempt and owning run/step bindings. These use the same execution model, not question/stage Session classes. A reply does not complete a different assignment or approve a decision. See [Session contract](sessions.md).

## Agent-first RPC and CLI

Both surfaces default to concise summaries and reliable write receipts. AppServer
owns filtering, pagination and response views; CLI requests `fields` / `view: "full"`
and controls compact JSON versus `--human` pretty formatting. Full content is
explicit. See the [current response contract](../agent-response-contract.md) for
method defaults, pagination, normalized comments, idempotency and breaking-client
migration. Durable event consumers retain their original fact payloads.

For example, `card.move` takes `card_id`, `revision`, `stage` and `request_key`;
its default result confirms the committed Card identity, stage and revision,
without echoing the description. It does not assert that a triggered Agent has
started. A handoff queue acknowledgement remains distinct from a recorded write.

## Identity and allowed actions

Distinguish human callers, named external agent clients, and the agent assigned to a particular run. The service derives authorship from the authenticated/local connection or run-scoped credential; a JSON author field is not enough.

A run-scoped agent should be able to read its task/context, comment, submit evidence, and report its result. Whether agents can also move stages, archive, change routing, or edit templates depends on declared permissions. The exact permission model is open; use explicit capabilities rather than equating a result report with human acceptance.

Preserve both agent profile and run identity for attribution. A generic profile name such as codex is not sufficient to identify which execution wrote a comment. Do not expose the full raw core API through untrusted preview content.

## Consistency and retries

- Mutations use scoped request keys and return stable operation identity. A repeated accepted command returns the same logical result.
- JSON-RPC message IDs only correlate messages; the request key is the application deduplication identity.
- Expected revisions prevent stale changes. Return structured conflicts with the current revision and a useful hint.
- A move and its canonical fact commit together. Durable subscriptions persist entry/delivery intent and cursor progress atomically; the accepted [delivery contract](events-and-notifications.md#transactions-and-delivery) does not depend on live channel receipt.
- Return observed states: accepted, queued, preparing, active, failed, unknown, or appropriate structured errors.
- An unknown terminal launch or initial-message outcome requires reconciliation; replaying a submitted mention returns its existing request rather than launching another conversation.
- Associate reports with their request/execution and callback. Artifact/check information can enrich context without becoming an independent completion gate. Do not require a full stale-process/resource-handover system for MVP.
- History and list responses need bounded pagination. Resource upload limits and content handling should be explicit in the eventual schema.

The current CLI and Unix-socket RPC boundary are implemented. The new workflow/authoring and durable subscription operations are implemented in the initial slice; see [concrete operations](workflow-engine.md). Use discoverable versioned schemas and the same validation for thin CLI/UI adapters. Durable event consumption differs from ephemeral invalidation and a paginated Timeline reader; see [subscriptions](events-and-notifications.md).

## Completion, blockage, and events

Use execution.report through the proposed Canban CLI so existing agents launched through Herdr can declare working, completed, blocked, or failed. The CLI reads the request/attempt association injected at launch; message/Markdown body are optional. See the [payload, receipt, and retry contract](../interfaces/mvp-contract.md#agent-report-rpc). A completed declaration directly drives the stored callback without a provider-specific evaluator or independent quality gate.

Blockage marks Card attention and generates an internal notification. The accepted durable event design commits facts/progress, with channels for wake-up/invalidation. UI/CLI mutations share that boundary. Native macOS notifications remain separately deferred; durable subscriptions are now specified in [events](events-and-notifications.md).

## Acceptance

A fixture agent should create a card, add a comment/resource, move it to Develop, observe the resulting request/run, submit a scoped result, and read the unified timeline using only the interface. Replaying any mutation must preserve card numbers and trigger counts. New declarative definitions must share Agent/manual validation and generic display; migration must preserve existing live/queued work and decisions. Project Workflows, SQL YAML history and Edit with agent use the same source/check/apply APIs. Local YAML files are explicit submission/export inputs, not monitored configuration. Onboarding remains open. API and UI operations must produce equivalent application outcomes.
