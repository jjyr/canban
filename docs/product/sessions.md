# One Session model, multiple scheduling triggers

Status: current user direction, 2026-11-05. Every agent execution uses the same Session model. @mentions and stage entry differ in how Canban schedules work, not in the nature or type of the session. Detailed storage and lifecycle choices below remain proposals.

## Agreed boundary

- A submitted @agent request starts a new conversation for the current question.
- Entering Develop or another executable stage independently selects an agent and starts a fresh conversation for the configured work.
- Do not introduce question-session and stage-session classes, separate managers, or different execution engines.
- Canban makes the scheduling decisions and controls execution through herdr-core.
- The same agent profile can be selected repeatedly; a fresh assignment means a new execution/conversation instance, not necessarily a different provider.

The earlier assistant proposal to append a normal @mention into the active Run is superseded. The subsequent proposal to classify sessions into question and stage types was explicitly rejected by the user. Use one common model.

## Product and execution relationship

```text
Submitted @agent comment ----+
                            |
Enter Develop --------------+--> Canban Scheduler
                            |      resolve task / agent / context / callbacks
Enter Review ---------------+      apply capacity and resource constraints
                            |                    |
Other configured trigger ---+                    v
                                            Session
                                      one common execution model
                                                 |
                                       herdr-core adapter
                                                 |
                                      core -> holder -> PTY
                                                 |
                                           agent process
                                                 |
                                       output/status/result
                                                 |
                                  Canban timeline + configured callback
```

Herdr-core does not need to know whether a terminal originated from a comment, a drag, or a future schedule. It receives the launch command, cwd, environment, task, and labels supported by its API. Canban retains the product meaning and applies the appropriate completion action.

## Trigger differences belong in Scheduler

| Trigger | Scheduler prepares | Completion behavior |
| --- | --- | --- |
| Submitted @agent comment | Addressed profile, current question, selected card/discussion/context revisions | Record the response against the initiating comment |
| Enter Develop | Stage workflow, tag/default agent selection, task/context, preparation callbacks | Agent-reported completion invokes the default Review transition |
| Enter Review | Review workflow, selected agent, actual change/result/check context | Record the configured report/outcome |
| Other configured trigger | Its execution specification and resources | Its configured result handling |

These are scheduling policies, not session types. A Session does not need a question/stage discriminator. The trigger event and outcome binding can live on the dispatch request.

Completion behavior must be bound to the request and initiating event. A response to a comment must not accidentally advance a Develop card as if it were the current development result. This validation uses request causality/current workflow ownership, not a different Session implementation.

## Keep Session and Run unified for MVP

Recommended terminology:

| Name | Meaning |
| --- | --- |
| Agent profile | Reusable configuration such as codex or claude, not a persistent shared conversation |
| Scheduler request | Durable work intent, trigger, captured context, routing/configuration, resources, and completion binding |
| Session / Run | One execution attempt with its isolated agent conversation; use the existing Run identity for MVP |
| Core Terminal | Runtime PTY/process identity bound to that execution |
| Provider conversation ID | Optional conversation ID from the agent tool; separate metadata when available |
| HC_SESSION | Core runtime/socket/state namespace; unrelated to an individual agent conversation |

Use one underlying attempt record for the session rather than inventing an additional Session hierarchy/table. The focused [Herdr ID research](../research/herdr-identifiers.md) recommends reusing (dispatch_request_id, attempt_number) as that logical execution key; run_id can be a serialized form rather than another independent UUID. A request may have retry attempts; each fresh attempt has its own execution key and conversation. This remains an implementation recommendation.

Suggested common fields: request_id plus attempt_number (optionally serialized as run_id), profile_id, context_snapshot_id, execution directory/resource bindings, operational state, timestamps, and result references. Reuse the owning core.id plus its local terminal.address as the optional runtime binding; no synthetic terminal ID is needed. Provider conversation ID is separate optional metadata when observed. Trigger/callback data are reachable through request_id. There is no session_type field.

The [reporting contract](../interfaces/mvp-contract.md) proposes injecting the Canban endpoint, request ID, attempt number, and CLI path at launch. The agent can report through that common interface without copying terminal/card IDs or knowing the next workflow stage. This keeps execution association concrete without adding another Session identity.

```text
Card #42
  +-- comment event M1 -> request Q1 -> Session/Run R1 -> terminal t1
  +-- comment event M2 -> request Q2 -> Session/Run R2 -> terminal t2
  +-- Develop entry E1 -> request Q3 -> Session/Run R3 -> terminal t3
  +-- Review entry E2  -> request Q4 -> Session/Run R4 -> terminal t4

R1 / R2 / R3 / R4 have the same structure and lifecycle.
Their requests carry different task inputs and callback bindings.
```

## Fresh agent conversation

Each new submitted @mention and genuine executable-stage entry creates a fresh work intent. When admitted, it starts a fresh conversation rather than reusing the currently active agent merely because the profile matches.

Recommend one primary core terminal per Run and an effective launch profile that does not silently resume a prior provider conversation. A new terminal alone does not guarantee this: provider startup behavior needs implementation and validation. Record provider conversation IDs where available without pretending the core terminal ID is that ID.

Reordering inside a stage is not stage entry. Replaying the same accepted mention or move returns/reconciles the same request and does not start another Session. A new deliberate mention has a new trigger identity.

An explicit later Continue/Reply-to-session capability can be considered separately. It is not the default behavior of @mention or stage entry in this working model.

## Context persists on the card

Human Context belongs to durable card/Note/resource/timeline data. Scheduler selects and captures it for each new execution.

```text
Card / Notes / decisions / previous artifacts
                       |
                captured input
                 /     |     \
               R1      R2     R3
                 \     |     /
                  comments/results
                         |
                   card timeline
```

Fresh conversations do not need to lose task history. They receive the relevant description, discussion, prior outcomes, and exact change references, with more context accessible through the application API. Do not silently inject every old transcript or resume hidden provider memory.

A fresh conversation does not imply a new repository or worktree. The execution specification can intentionally reuse a prepared checkout/artifact from development for review while still starting a fresh agent conversation.

A submitted @mention selects a profile for that request; it does not permanently retag the card or replace the next stage's agent routing. It may refer to an earlier Run as context, without sending the prompt into that Run.

## One lifecycle and one admission path

```text
request queued
      |
      v
Run preparing -> launching -> active -> collecting -> finished
                      |          |
                      v          +-> input needed / cancellation requested
                    unknown
                      |
                  reconcile
```

All Sessions use the same ordinary global/Project/profile capacity controls, observation, cancellation, and report handling. Task/context/callback configuration can differ without requiring separate session classes or a resource-lease framework.

MVP uses ordinary logical-work capacity controls without a session-type distinction. The user explicitly accepts exceptional residual old execution, including mentioned agents. Do not add exclusive-writer leases, process-drain, or coordinated handover as progression gates. More elaborate resource/overlap policy can follow MVP.

The agent's explicit completed declaration finishes logical work and invokes the request callback. It need not prove terminal exit or independently verified quality. The next stage can proceed even if the previous process exceptionally continues. A fresh conversation can reuse the card's worktree, which is retained until Archive.

## Reconnect, retry, and retention

| Situation | Behavior |
| --- | --- |
| Viewer/network reconnect | Reconcile/reattach the existing Run and terminal |
| Core restart with surviving agent process | Preserve the Run after runtime identity checks |
| Launch reply lost | Mark unknown and reconcile; no blind duplicate Session |
| Retry after effects/failure are resolved | Recommended: new Run and fresh conversation, old history retained |
| Program recovered as a fresh shell | Original agent conversation has not resumed |
| Late result from obsolete request | Append attributable history without regaining stage-advancement authority |
| Card archive / core terminal cleanup | Retain Canban execution/context/result history; Archive is the managed-worktree handling boundary |

On Card archive, the user selected zstd/lz4 compression of related Card and Agent execution data/results. Retained Canban execution records, captured input, and reports can move into the Card archive package and remain accessible on demand. This does not move or compress Herdr's own runtime store. See [compressed Card archives](../data-architecture.md#compressed-card-archives).

Do not create one HC_SESSION per conversation:

```text
HC_SESSION=default -> one core runtime
                      +-- terminal t1 -> Run R1
                      +-- terminal t2 -> Run R2
                      +-- terminal t3 -> Run R3
```

Core namespace behavior is documented in the pinned [paths implementation](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/paths.rs#L8-L35); [terminal records](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/terminals.rs#L13-L48) are runtime objects. Core does not provide a Canban conversation manager.

## Attention and internal events

Use Herdr's existing runnable agents/configuration through core. A blockage/needs-input report produces a card needs-attention condition and an internal notification event. The user chose Rust enum events, an in-memory channel, and topic subscribers as the product direction; macOS delivery and detailed event architecture come after MVP. See [events and notifications](events-and-notifications.md).

## Acceptance checks

- Two distinct @codex submissions produce distinct fresh conversations; replaying one submission produces one request.
- Develop entry independently selects a profile and launches the same generic Session implementation with development task/callback data.
- Default Review entry launches a fresh conversation with the development artifact rather than resuming the developer conversation.
- A comment response and a development result use their respective request-bound completion actions without Session type branching.
- Ordinary capacity, cancellation, reporting, and event handling use the same generic path for every trigger; no residual-process handover gate is required.
- Reconnect retains the execution; a deliberate resolved retry gets a fresh Run.
- The launch profile cannot silently resume a previous provider conversation.
- Timeline/context remain readable after archive or core terminal cleanup.
