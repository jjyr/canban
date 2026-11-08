# Crate boundaries and card timers

Status: current naming/crate direction and timer placement proposal, 2026-11-06. The user selected AppServer and Workflow Engine, proposed Scheduler for queue/execution/time responsibility, and asked for crate separation. Timer placement below is a proposal; adding this boundary does not automatically add recurring scheduling to MVP.

## Names and responsibilities

| Component | Cargo package | Directory | Responsibility |
| --- | --- | --- | --- |
| AppServer | canban-app-server | crates/app-server | Shared operations, composition, lifecycle, adapter entry, and routing to module owners |
| Kanban | canban-kanban | crates/kanban | Projects, Boards, cards, stage/attention state, and card timing attributes |
| Workflow Engine | canban-workflow-engine | crates/workflow-engine | Interpret JSON templates and advance workflow through configured actions |
| Scheduler | canban-scheduler | crates/scheduler | Routing/default selection, queue/readiness, timed triggers, and generic Session execution |
| Agent profile configuration | canban-profiles | crates/profiles | Resolve editable JSON presets, command overrides, generic expansion and legacy configuration conversion |
| Context | canban-context | crates/context | Notes, resources, attachments, and captured execution input |
| Timeline | canban-timeline | crates/timeline | Durable card history/comments/reports and history queries |
| Notifications | canban-notifications | crates/notifications | Internal notifications/events; later native delivery |
| Herdr adapter | canban-herdr | crates/herdr | Core client protocol, terminal binding, launch/observation/control |
| Native Terminal | canban-terminal | crates/terminal | libghostty-vt state/input and AppKit viewer; Herdr stream binding, without Card/workflow/storage dependencies |
| Workspaces | canban-workspaces | crates/workspaces | Directory/Git/worktree preparation/reuse, Review preview/commit/merge and Archive handling |
| Storage | canban-storage | crates/storage | Shared local persistence/attachment IO implementations |
| Shared boundary types | canban-types | crates/types | Only shared IDs, command/event/report/action payloads needed across crates |
| UI Kit | canban-ui-kit | crates/ui-kit | Generic tokens, styled reusable NSH controls, and shared interaction conventions; no product/service dependencies |
| UI/desktop host | canban-ui | crates/ui | Reusable product UI library, presenters/screens, UI state and independent-service desktop client; standalone Gallery imports it from examples/gallery |
| Agent CLI | canban-cli | crates/cli | Thin agent-facing adapter to AppServer operations |

This layout is now represented by workspace crates in the initial MVP. Core components have compile-time boundaries and run in one Canban process; the CLI also provides a headless host without linking NSH UI. Timer internals and future capabilities below remain proposals. See [implementation and acceptance](implementation.md).

The user selected prefixed Cargo package names but unprefixed filesystem names. For example, crates/scheduler/Cargo.toml declares canban-scheduler, while its source module is src/timers.rs. Follow the [development conventions](development-conventions.md); Canban's project license is Apache-2.0.

The shared-types crate is a narrow dependency boundary, not a place for all business code. Exact supporting-crate granularity can be adjusted during implementation while preserving the major AppServer/Kanban/Workflow Engine/Scheduler/UI boundaries.

The user subsequently approved the separate UI Kit/product UI boundary and component/gallery/regression conventions in [UI architecture](ui-architecture.md). canban-ui depends on canban-ui-kit; UI Kit builds on NSH and must not depend on AppServer or business-domain crates. Individual components use ordinary modules, not individual crates.

The request/attempt data model is unchanged by renaming the scheduling component. Earlier dispatch_request_id notation can remain a logical request key; renaming does not require another execution/session identity or wire-schema migration before an implementation exists.

## Runtime organization

```text
canban-ui: native Rust/NSH
          |
          v
canban-app-server <--- IPC adapter <--- canban-cli
          |
          +-- canban-kanban
          +-- canban-context
          +-- canban-workflow-engine
          +-- canban-scheduler
          +-- canban-timeline
          +-- canban-notifications
          |
          +-- shared event channel / operation routing
          +-- storage / workspaces / herdr adapters
                                      |
                                  herdr-core
```

AppServer wires components and routes operations/actions. Template progression belongs to Workflow Engine; queue/time and execution belong to Scheduler. Keeping these policies out of the entry layer avoids making AppServer the whole workflow implementation.

Implementation update, 2026-11-09: UI clients attach to a persistent AppServer, started by desktop startup or `canban serve`. One authority owns scheduling across execution machines. UI exit does not stop it. See [multi-machine execution](multi-machine.md).

## Rust dependencies must stay acyclic

The runtime may contain a feedback loop such as stage change -> Workflow Engine -> Scheduler -> result -> Workflow Engine -> stage change. That does not require a circular Cargo dependency.

```text
canban-ui ----------------------> canban-app-server
                                        |
                         +--------------+--------------+
                         v              v              v
                  canban-kanban  canban-workflow-  canban-scheduler
                                      engine
                         |              |              |
                         +--------------+--------------+
                                        v
                                  canban-types

canban-app-server also wires:
  storage / context / timeline / notifications / workspaces / herdr

canban-cli -> shared wire/types + local IPC client
future daemon host -> canban-app-server (no NSH dependency)
```

Dependency arrows above are a minimum direction sketch, not an exhaustive manifest. Supporting crates depend on shared types and applicable infrastructure; they must not import the UI or depend back on AppServer. Workflow Engine and Scheduler do not need to import one another: they can consume shared event data and produce typed actions wired by AppServer.

A practical implementation can expose pure/template decisions plus async module operations. The exact event publisher/subscriber traits and channel crate remain deferred. Separate crates do not require a mailbox, thread, actor, or process per crate.

## Where Timer belongs

Separate three concerns:

| Concern | Owner |
| --- | --- |
| Card's configured time/intent, such as remind_at or run_at | Kanban persists it as card-associated product data |
| Clock waiting, due-time detection, and making timed work ready | Scheduler, initially a timers module inside canban-scheduler |
| What to do when time arrives | The configured action: Notifications for a reminder; Workflow Engine for a workflow action |
| Timer/date/countdown input and display | Rust/NSH UI, calling AppServer for saved changes |

Recommendation: do not create an independent Timer engine/crate for the initial design. Time is one way a Scheduler trigger becomes ready. Keep its runtime implementation inside Scheduler unless a later real need justifies extracting a reusable timer component.

A timer must not depend on a particular NSH view staying mounted. The UI is an editor/view of the timing configuration, not the owner of when business work fires.

## Timed workflow action

Illustrative control flow; names are not a finalized event enum or API:

```text
User sets a card execution time
       |
AppServer -> Kanban saves timing configuration
       |
card timing update event
       |
Scheduler registers/updates a due-time trigger
       |
time arrives -> timer-fired event
       |
Workflow Engine applies the configured template action
       |
       +-- request Kanban stage change, if configured
       +-- request Scheduler to queue an execution
                              |
                        generic Session
                              |
                          herdr-core
```

The two action branches in the diagram are alternatives. If the configured action moves a card into an executable stage, use that stage's existing scheduling trigger; do not separately enqueue the same work a second time.

The timer does not directly choose that every card must enter Develop or run an agent. Workflow Engine defines the relevant workflow action from configuration. Scheduler later starts the resulting execution under the same scheduling path as other work.

A queue may also support not-before eligibility for a task that is already specified. That is a Scheduler readiness concern. Exact context-capture timing and behavior if the card/template changes before firing remain product refinements for implementing timed work; this document does not silently choose them.

## Reminder versus scheduled execution versus deadline

A time field alone is ambiguous. Keep the intended action explicit:

- **Reminder:** when it expires, create an internal notification associated with the card. It does not automatically start an agent.
- **Scheduled execution/workflow action:** when it expires, activate its configured workflow action through Workflow Engine and Scheduler.
- **Due date/deadline:** metadata for display/ordering or an explicitly configured reaction; it is not automatically a launch request.
- **Display countdown:** local UI presentation updates. It does not make a view-local ticking timer the authority for business scheduling.

These are timing intents/actions, not new Session types. All resulting agent work uses the same generic Session mechanism.

## Lifetime and MVP boundary

Saved timing configuration belongs in persistent product state. An in-memory wait/registration is runtime state that can be rebuilt from it when the owner starts. This records ownership only; detailed missed-run, timezone/DST, recurrence, and replay policy remains deferred until the corresponding feature is implemented.

If AppServer itself stops, its Scheduler does not run even if Herdr terminals survive. Closing a UI leaves the independent service running. Timer placement does not imply recurring-job support.

The earlier MVP excludes advanced scheduled-task management. Discussing where a card timer belongs does not commit to shipping cron, recurrence, a Timer service, or always-on execution now. The existing blocked-card internal notification remains in MVP, independently of timed reminders.

See [overall architecture](architecture.md), [internal comparison](architecture-internal-options.md), [event direction](product/events-and-notifications.md), and [MVP plan](mvp-plan.md).
