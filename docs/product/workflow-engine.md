# Unified workflow engine

Status: user-confirmed direction with the initial implementation added 2026-11-11. See [concrete operations, supported semantics and evidence](../workflow-implementation.md). There is one interpreter for converted templates and custom definitions. The rules below distinguish the overall contract from still-open richer UX/schema extensions. [Events/subscriptions](events-and-notifications.md) owns trigger delivery.

## Decisions

- Keep Backlog / Develop / Review for the current Board. Do not add a Design column.
- Separate Board position from workflow progress. Planning and plan approval can happen while a Card remains in Backlog.
- Use one Workflow Engine for current development/review, manual starts and event-triggered automation. Do not retain a second stage-callback executor.
- Reuse Agent Profiles, Card/profile Conversations, Requests, Attempts, Scheduler, workspaces and Herdr execution.
- Agents should author and modify workflow definitions. Users can also edit the same declarative source manually. Both paths use shared validation and revision handling.
- Provide one generic UI for any supported workflow: definition, progress, results and pending decisions. A custom screen per workflow is not the model; a visual graph editor is not required.
- Project Workflows owns multiple YAML sources, original text and immutable versions in SQLite. No Workflow files or revision mirrors are generated in ~/.canban; files are only submission/export transport.
- Edit with agent previews/copies one Project-wide Prompt and launches/continues a Project-owned configuration conversation in Terminal. Column trigger icons use a client-side library to describe active rules. Onboarding remains open.

## Model and ownership

| Object | Responsibility |
| --- | --- |
| Board / column | Card business position; stable IDs independent of display names |
| Workflow definition | Versioned entry points, steps/transitions, input/output references, Agent references and workspace policy |
| Subscription | Select committed events by scope/type/filter and target an internal consumer or workflow entry |
| WorkflowRun | One invocation with a captured definition revision, initiating event/command, progress, results and decisions |
| Step execution | One visit to a definition node; rework creates a new execution identity |
| Request / Attempt | Existing assignment/execution identities for an Agent step |
| Approval | Decision bound to run, step execution and exact result/input revision |

```text
Project
  +-- Board: Backlog / Develop / Review
  +-- workflow definitions / subscriptions
  +-- Card
       +-- content / source binding / Timeline
       +-- WorkflowRun
            +-- captured definition revision
            +-- step executions / outputs / decisions
            +-- Agent step -> Request -> Attempt
                                  |
                          Card/profile Conversation
                                  |
                         Herdr / provider history
```

WorkflowRun is a product process identity, not a provider Session type. A definition is not a live conversation. Preserve the [Conversation/Request/Attempt contract](sessions.md); a reply cannot complete another assignment or step.

AppServer composes operations/transactional IO. Kanban owns Card transitions. Workflow Engine owns definition validation, entry/run/step interpretation and decision progression. Scheduler owns readiness/admission and existing execution lifetimes. Storage owns journals, deliveries and checkpoints. Timeline/Notifications/UI consume shared facts. These boundaries do not require a process/thread per component.

## One execution path

Manual invocation, subscribed events and external column entry address named entry points in the same model. An Agent workflow without a Card must resolve/create one through configured intake/product operations. Repository observer subscriptions can exist without a Card.

```text
manual start ------+
subscribed event --+--> durable entry intent --> WorkflowRun
external move -----+                                |
                                             Workflow Engine
                                                   |
                          +------------------------+---------------+
                          |                        |               |
                      Card action             wait / branch   Agent request
                                                                   |
                                                               Scheduler
                                                                   |
                                                            existing runtime
                                                                   |
                                                            explicit report
                                                                   |
                                                            owning step
```

Agent steps queue Requests; event handlers do not launch terminals directly. Scheduler retains the global budget, conversation reuse, runtime reconciliation and unknown-launch behavior. Engine decides what should happen; Scheduler decides when Agent execution starts.

By default a Card's main delivery process has one active run. Overlapping triggers must resolve to the existing run, be rejected or wait under an explicit policy; they cannot silently create competing development pipelines. Independent observer subscriptions remain possible. The implementation queues competing event receipts without consuming failure retries while an active run/human gate holds the Card; receipts capture their definition revision.

## Definition and progression rules

The initial vocabulary covers Agent execution, Card movement, conditions, human approval, internal notification and explicit completion. Input/output references connect results to later steps. The implemented YAML vocabulary uses `agent`, `card.move`, `condition`, `approval`, `notify` and `complete`, compiled into the existing interpreter.

- Stable step IDs identify nodes; each visit has a distinct execution identity. Rework preserves earlier plans/reports/decisions.
- Validate entry points, transition targets, typed parameters, data references, column IDs and Profile references before publishing a revision. Reject unsupported step kinds for execution.
- Conditions use declared data and a constrained expression model. Arbitrary executable callbacks are not required for authoring.
- Capture the definition revision at run creation and effective Profile/task configuration at assignment creation. Edits affect new runs, not active runs or already captured assignments.
- Advance an Agent step from its explicit bound result. Terminal idle, root `done`, process exit, unrelated comments and generic replies do not establish completion.
- Trust the Agent's completion declaration; do not add an independent quality evaluator or process-drain prerequisite. Validation work can be an explicitly configured task.
- Blocked/failed reports and uncertain execution retain actionable failure/decision state. Retry only under a declared safe policy; unknown launch/write outcomes require reconciliation.
- Persist accepted results and the resulting step/action intents consistently. Repeated reports/deliveries cannot complete a step twice.
- Rework is a recorded transition with a new execution/input identity. Human-requested rework can revisit a step; reject unbounded automatic cycles and impose explicit execution/trigger limits.

Parallel branches/joins, arbitrary scripting and reusable subflows are not prerequisites. The first model can express sequences, conditions and explicit rework. The viewer reflects supported semantics rather than implying unsupported execution features.

## Three columns and example entries

```text
Board position:  Backlog                       Develop           Review
                    |                            |                 |
Workflow:      plan -> optional approval -> move -> implement -> move -> review
                                                                         |
                                                                   human decision
```

Backlog entry has no built-in Agent action. A separate manual/event entry can run planning on a Backlog Card. This supersedes treating non-executing Backlog as a ban on all Agent work while a Card is there.

Default delivery preserves external move to Develop -> developer -> move to Review -> reviewer -> human decision. An eligible external move directly to Review can enter the review portion. A planning entry adds plan/optional approval before delivery. Whether these are named entries in one definition or separate definitions sharing steps is a schema choice; there is one runtime either way.

A workflow-owned move emits the normal Card event with run/step causality. Entry matching continues that run instead of launching another developer. Manual movement away from expected progress must explicitly reconcile/supersede affected steps, cancel stale queued work and retain running work/results as history. Leaving and returning to the same column cannot validate an old completion.

Required approval cannot be silently satisfied by dragging, reading notifications or another completion. A user-directed rework/move that changes the process is a recorded decision; the future UI must show its consequences.

```text
GitHub observation
       |
existing configured intake
       |
Card created in Backlog, source binding committed
       |
subscription: automatic Issue intake after activation baseline
       |
planning entry -> read source/code -> plan artifact
       |
       +-- configured automatic continuation ----------------+
       |                                                     |
       +-- approval -> approve exact plan --------------------+
                  \-> request changes -> new plan             |
                                                             v
                                                       move to Develop
                                                             |
                                                       development task
                                                             |
                                                       move to Review
                                                             |
                                                       review / human decision
```

Intake itself creates ownership in Backlog; execution requires an enabled matching subscription. Activation defaults to future events; existing Card/history processing is explicit. Manual start addresses the same entry/run model.

The plan is a durable Agent-authored artifact/report. Development receives the selected/approved result, source identities and local context. Remote prose still follows the [GitHub cache contract](../github-workspace.md); reading a source does not silently archive all remote bodies.

## Approval and result validity

Approval binds to run, step execution, exact plan/result and relevant input versions. It survives restart and is consumed once. Required human approval uses user provenance; Agent completion/configuration authoring cannot manufacture a human decision. Configuring automatic continuation is separate from approving an already waiting run.

Record which input fields/versions affect a result. Relevant changes mark the result/approval stale and follow a declared replan/wait policy before progression. Unrelated comments/read acknowledgements must not invalidate every result. Dependency declaration syntax and conflict presentation remain schema/UX work.

Reports keep request/attempt binding and resolve to their owning run/step execution. Old results stay readable but cannot advance a superseding step, a different stage entry or a stopped run. Stopping future progression and terminating a live Agent are distinct effects; canceled workflow state is not proof of process termination.

## Agent authoring and generic display

```text
manual source edit --------+
                           +--> same declarative definition
Agent-assisted edit -------+              |
                                shared validation / revision handling
                                          |
                                    Workflow Engine
                                          |
                                generic definition/run viewer
```

Agent authoring is a primary customization path, not a separate execution language. Provide a machine-readable schema and useful diagnostics. Manual and Agent edits share expected-revision checks. Invalid drafts cannot replace a valid active definition. Save and apply atomically publishes original YAML, active revision references and named trigger subscriptions. It does not start/replay history or approve existing gates. Old runs/captured deliveries remain frozen; future matching events use the new revision.

The generic UI renders entry points, steps/transitions, selected path, progress, Agents, inputs/outputs, decisions, failures and causal history from the shared model. Unsupported types may use a preserved-data fallback for viewing, but cannot execute as understood types. Default delivery and plan-and-develop must not need separately hardcoded workflow screens. Users edit declarative source or ask an Agent to modify it; a graph editor is not required.

Project Workflows and Settings link to Edit with agent. Its configuration conversation is Project + purpose + Profile, without a fake business Card. Full selectable Prompt preview, explicit copy, Start/Continue/Resume, native Terminal and a return link are implemented. Manual editing and history share SQL publication APIs. Onboarding/default Profile setup and provider authentication remain separate. Configuration authoring cannot approve a pending human gate.

## Migration: one runtime

| Current representation | Unified representation |
| --- | --- |
| Stage `execute`, `profile`, `instruction` | Agent step |
| Stage `on_completed` | Definition transition / Card move |
| Stage `require_human_review` | Approval step after the result |
| Stage entry | Named workflow entry with event causality |
| Workflow workspace settings | Policy inherited by execution steps |
| Request callback | Owning run/step and captured transition |
| Existing human gate | Pending workflow decision and Card attention projection |

Legacy JSON may convert at load/migration time; converted and new definitions execute through the same engine. Remove direct callback progression and duplicate stage-entry enqueueing when the engine takes ownership. Compatibility parsing must not become a second runtime.

Associate queued/running Requests and pending gates with run/step checkpoints without relaunching Agents, recreating worktrees, changing provider IDs or replaying completion. Preserve captured old configuration. Migration must represent the whole active state or roll back, never attach the engine to a partially converted queue. Follow paired transactional database migrations and versioned configuration publication. Historical records keep their meanings.

## Future acceptance requirements

1. Manual/subscribed starts use the same run model; duplicate activation creates no duplicate work.
2. Planning and approval run in Backlog with exactly three Board columns.
3. Default Develop/Review uses the engine alone; workflow-owned moves do not duplicate Agents.
4. Approval/version changes, stop/rework and leave-and-return reject stale progression while preserving history.
5. Restart resumes durable work once; unknown external effects await reconciliation.
6. Existing active/queued tasks and human gates migrate without new launches or lost decisions.
7. Agent-authored/manual definitions share validation and a generic viewer.

These are implementation acceptance requirements, not newly performed checks. Concrete Project authoring and native interaction acceptance is tracked in the implementation reference; onboarding remains open.
