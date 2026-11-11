# Comparing Canban's internal organization

Status: architecture discussion/proposal, 2026-11-05. The external boundary is already clear: native Rust/NSH UI plus a logically independent AppServer, co-hosted for MVP; herdr-core is a separate runtime. This comparison concerns the modules inside that AppServer, not another process split or new Session types.

Historical comparison: AppServer is now an independent service. The accepted 2026-11-11 [workflow engine](product/workflow-engine.md) and [events/subscriptions](product/events-and-notifications.md) supersede this document's event-deferral and stage-only examples. Retain these alternatives as design history, not current runtime/delivery contracts.

Keep the user's simplifications: agent-declared completion, ordinary Review report, JSON templates, worktree handling at Archive, no old-execution coordination, and only minimal enum/channel/topic infrastructure for MVP.

## Common components

All alternatives can use the same product modules and storage. The question is how work flows between them.

| Component | Responsibility |
| --- | --- |
| AppServer | Shared command/query/report entry, composition, and typed access for UI/adapters |
| Kanban | Projects, Boards/stages, cards/numbers, and authoritative card transitions/attention |
| Context | Card resource references, attachments, and captured input; legacy Note data retained |
| Workflow Engine | Interpret JSON templates and turn stage/report events into configured actions |
| Scheduler | Routing/default selection, request/attempt lifecycle, and generic Session execution |
| Workspaces | Prepare/reuse execution directory/worktree; handle managed worktree at Archive |
| Timeline | Card history/comments/report presentation and persistence access |
| Notifications | Internal notification records/events; future native-delivery adapter |
| Core adapter | Launch/read/observe/control through herdr-core |
| Store | Shared persistence boundary for local state/history/attachment metadata |

The user subsequently requested crate separation. Major responsibilities therefore have dedicated crate boundaries, as proposed in [crates and timers](architecture-crates-and-timers.md). This comparison still concerns control flow: separate crates do not imply separate actors, threads, databases, or processes. Workflow Engine rules and Scheduler execution remain different owners.

## A: explicit service orchestration

```text
NSH UI / CLI / future adapter
                 |
          AppServer
                 |
          Workflow Engine coordinator
           /      |      \
      Kanban   Context   Scheduler
         |                  |
      Timeline          Workspaces
         |                  |
   Notifications        Core adapter
           \                /
                 Store

Events -> UI updates / observers
```

The coordinator directly calls the modules in an explicit sequence: change card -> prepare execution -> launch agent -> handle report -> move to the next stage. Internal events primarily announce the results to interested observers.

Strengths: the main sequence is easy to follow, result/error return paths are direct, and a small workflow can be implemented with little messaging.

Tradeoff: the coordinator grows as more triggers, reporting actions, notifications, and integrations are added. Unrelated effects can accumulate in the same orchestration path. Each new reaction tends to require editing that path.

This is a valid simple implementation. It does not by itself provide the modular subscription style the user wants across the product.

## B: module ownership with event-driven reactions

```text
NSH UI -------- typed Rust calls ---+
CLI/RPC ------- adapter ------------+--> AppServer
                                             |
                                  route operation to owner
                                  /           |          \
                              Kanban       Context     Scheduler
                                  |           |          |
                                  +-----------+----------+
                                              |
                                     small AppEvent enum
                                     in-memory channel
                                              |
                 +----------------------------+------------------+
                 |                |              |               |
             Workflow Engine         Timeline      Notifications     UI/read views
                 |
          configured action
          /              \
     Kanban command    Scheduler.enqueue
                            |
                        Workspaces
                            |
                        Core adapter
                            |
                        herdr-core

State/history IO -> shared Store boundary
```

Each module owns its own decisions and authoritative updates. Events announce changes/results; interested components react. A reaction that needs to change another module's state uses that owner's operation rather than writing its fields/database rows directly.

AppServer is the common entry/composition boundary, not a requirement to put all product logic into one giant struct or mutex. The same-process UI can call its typed handle; adapters reach the same operations.

Strengths: aligns with the user's event-driven direction; notification, Timeline, UI, and workflow reactions can evolve without becoming one large coordinator function. The native UI and future daemon reuse the same service/modules.

Tradeoff: an event chain is less locally visible than a direct sequence. Ownership and causality must be clear, and the same effect must not be produced by several subscribers.

Recommendation: **use B for the MVP's logical module structure**, with a very small implementation. Start with only the enum/channel/subscriptions needed by the normal product loop. A logical module may be an ordinary Rust function/component in the same execution context; this does not require an actor runtime.

## C: independently running component actors

```text
UI / adapters
      |
   service handle
      |
  +---+----------------+----------------+----------------+
  |                    |                |                |
Kanban actor      Workflow Engine actor   Scheduler actor    Context actor
own mailbox/state own mailbox      own mailbox      own mailbox/state
  |                    |                |                |
  +---------------- message communication ---------------+
                       |
              Timeline / notification actors
                       |
              Store / core / filesystem adapters
```

Each component has an explicit mailbox, state lifecycle, and independently scheduled processing loop. Request/reply, queries, cancellation, and component shutdown are message protocols.

Strengths: explicit concurrent state ownership and isolated component processing can be useful when components have materially different concurrency/lifecycle needs.

Tradeoff: adds mailbox, request/reply, snapshot, ordering, shutdown, and supervision concerns before the core product loop needs them. Cross-module state transitions and queries require more coordination.

This remains an option for a component whose later workload justifies it. The fact that the product is event-driven does not require every component to begin as an actor.

## Comparison

| Concern | A: explicit coordinator | B: modular event reactions | C: component actors |
| --- | --- | --- | --- |
| Main sequence visibility | Direct function/call sequence | Follow command -> fact -> reaction | Follow mailbox messages and replies |
| Business ownership | Modules plus a coordinator | Explicit module owner; other modules request changes | Actor owns state and mailbox |
| New Timeline/notification reaction | Coordinator hook or observer | Add a small subscriber | Add/wire actor messages |
| Concurrency machinery | Background IO tasks as needed | Background IO tasks as needed | Independently scheduled loops and protocols |
| Service extraction to daemon | Possible with UI-independent boundary | Possible with same boundary | Also possible; actors do not themselves define process placement |
| MVP fit | Smallest direct flow | Best fit for the agreed module/event direction | More lifecycle machinery than currently required |

The recommendation is B, keeping A's useful local clarity inside each module. C can be introduced selectively later, rather than imposed across the application now.

## Ownership rules for B

- Kanban is the writer of card stage/attention state. Workflow Engine asks it to move/mark a card; a notification subscriber does not secretly update the same row.
- Scheduler owns request/attempt/Session state and the core binding. UI views and Timeline do not independently start agents.
- Workflow Engine reads the stored JSON binding and chooses the next action. It does not introduce a second Session implementation.
- Context owns references/input capture; UI owns transient focus/selection/draft presentation. Notes are [deferred](notes-deferred.md); saved legacy data remains compatible without a Note product surface.
- Timeline owns the history view/recording boundary; it does not decide workflow progression.
- Notifications owns internal notices/delivery, with macOS delivery later. It can request an owner operation for card attention when appropriate.
- Storage is not another independent source of business decisions. Modules use one shared persistence boundary; separate modules do not imply separate databases.

A command is an intent handled by its owner; an event describes what happened and may have multiple interested subscribers. These roles clarify responsibility without choosing a complete command bus, event framework, or topic implementation.

The diagrams do not specify storage commit order. Required durable card/history data must remain persisted; a memory channel is not a durable log. Exact history persistence coupling, delivery semantics, topic design, and subscription API remain implementation/deferred-event details, not a requirement to build event sourcing.

## Normal development flow in B

Event/operation names below are illustrative, not a finalized enum/schema.

```text
1. NSH UI -> AppServer -> Kanban.move_card(Develop)
2. Kanban updates card -> CardMoved event
3. Workflow Engine reads Develop binding -> Scheduler.enqueue(task specification)
4. Scheduler selects profile -> Workspaces.prepare_or_reuse
5. Scheduler -> CoreAdapter.launch -> herdr-core terminal
6. Agent -> CLI/report entry -> Scheduler records completed declaration
7. Scheduler -> ExecutionReported event
8. Workflow Engine applies that request's on-completed binding
9. Workflow Engine -> Kanban.move_card(Review)
10. CardMoved -> same mechanism schedules the ordinary Review report
```

The agent decides completion. No test/quality/process-exit gate or old-execution handover is added by this module organization. Fresh @mention work follows the same Scheduler/Session path with different request/callback data.

## Blocking and notifications in B

```text
Agent reports blocked
        |
Scheduler records report -> relevant application event
        |
        +-- owner operation: Kanban marks card needs attention
        +-- Notifications creates internal notice/event
        +-- Timeline records relevant task history
        +-- UI observes updated card/notification views

Later: macOS notification adapter consumes internal notice
```

Keep one designated owner/effect handler for each update. The same event can inform several components without giving all of them responsibility to create the same notification or move the same card.

## What to settle now

The user refined the component names to AppServer, Workflow Engine, and Scheduler and requested actual crate separation; the [crate plan](architecture-crates-and-timers.md) records that direction plus Timer ownership. Detailed Rust channel/topic APIs, full event catalog, actor supervision, distributed delivery, and thread partitioning remain deferred.

This comparison is a proposal for discussion, not a completed implementation. See [overall architecture](architecture.md), [event direction](product/events-and-notifications.md), and [MVP interface contract](interfaces/mvp-contract.md).
