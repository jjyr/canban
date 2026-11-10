# Event-driven product direction and notifications

Status: user-confirmed MVP direction, 2026-11-05. The user explicitly deferred detailed event architecture until after MVP. This document records the boundary and product behavior, not a full event framework design.

## Direction

Internal product activity produces events. Represent application events as a Rust enum, carry them through an internal in-memory channel, and allow interested components to subscribe by topic. Use this direction to drive the product.

```text
UI actions / agent API calls / agent reports / runtime observations
                              |
                        Rust event enum
                              |
                       in-memory channel
                              |
                         event topics
            +-----------------+-----------------+
            |                 |                 |
      workflow/Scheduler    card/Board UI      timeline recorder
            |
            +-----------------+
                              |
                    internal notifications
                              |
                    future macOS subscriber
```

The diagram shows interested consumers, not a finalized process/thread or dependency layout. Channel library, exact enum variants, topic taxonomy, subscription API, ordering/delivery guarantees, replay, and any advanced infrastructure are deferred. Start with the smallest enum/channel/subscription implementation needed by MVP.

## Timed events

Card timing configuration is product data owned by Kanban. Scheduler owns clock waiting/due-trigger readiness, while Workflow Engine interprets a configured workflow action and Notifications handles reminders. Timer UI is an editor/view of this configuration. This is an ownership boundary only, not a selected timer/event framework or a decision to add advanced schedules to MVP. See [crates and timers](../architecture-crates-and-timers.md).

## Completion events

The [MVP interface contract](../interfaces/mvp-contract.md) defines a concrete proposed external reporting entry point and launch-context association now. That small report/receipt boundary is separate from the deliberately deferred detailed internal event architecture.

An agent explicitly reports whether it has completed. The completed event invokes that request's configured workflow callback; default development completion advances to Review and triggers the ordinary report workflow.

MVP trusts that report for progression. It does not independently verify quality or wait for process termination. Exceptional old executions that continue are outside MVP coordination scope.

The completion of a mention-triggered request and a stage request use the same event/session mechanism with different callback data. This does not create Session types.

## Needs attention and internal notification

The global [Agent Overview](overview.md) contains all running/waiting/queued/idle/closed conversations across Projects. [Attention handling](needs-attention.md) defines distinct task-decision/runtime-wait labels, context/navigation and independent resolution. [Unified reminders](../reminders.md) now combines pending actions with durable unread Agent updates for Card/Project/navigation counts and local sound delivery. Reading does not resolve an action. Native macOS notifications remain deferred.


When an agent reports blockage or needs a human response:

1. Produce an application event for that condition.
2. Mark the associated card needs attention without inventing a separate mandatory Board column.
3. Record the relevant card/session detail in its timeline.
4. Produce an internal notification event associated with that card.
5. Let interested UI/notification components consume it so the user can find the task.

The exact enum names and notification presentation remain implementation details. Native macOS notifications are a later integration, not a first-release dependency. An internal notification is an event even before native delivery exists.

An explicit response or reported resolution can update attention through the same event mechanism. Do not silently infer that merely opening a card solved the agent's problem.

## Memory transport and durable history

The in-memory channel distributes events inside the running application. The card's timeline remains durable product data, as previously required. A timeline consumer can persist relevant card changes, comments, reports, and notifications.

Do not confuse event transport with persistence or require every transient internal event to become a complete event-sourcing log. The existing [timeline format](timeline.md) is a history/payload draft, not the finalized Rust event enum, topic catalog, or channel protocol.

## MVP checks

- A card action or agent report emits an internal event observed by the relevant component.
- Agent completion drives the configured normal progression and timeline updates.
- Agent blockage produces needs-attention state and an internal notification.
- The UI can navigate from the notification to the affected card.
- Card history survives restart without claiming the memory channel is durable.
- The absence of a macOS notification adapter does not block the internal notification flow.

The event architecture will be revisited after the core Kanban + Scheduler MVP works. Do not make detailed infrastructure design a prerequisite to delivering that loop.
