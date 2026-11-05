# Scheduler and agent-reported progression

Status: user-directed MVP behavior, updated 2026-11-06 for the Scheduler component name. Configuration/API examples remain design drafts, not implemented features.

## One scheduling path

```text
stage entry / submitted @mention / configured action
                         |
                  application event
                         |
                   Canban Scheduler
          task + profile + context + callbacks
                         |
                 ordinary capacity queue
                         |
                 fresh generic Session
                         |
              herdr-core terminal execution
                         |
                 agent reports outcome
                         |
          event -> timeline / callback / attention
```

Stage defines what work to do, tags/defaults select who does it, and Scheduler decides when logical work can start. All requests use one Session model and execute through core.

## Routing

- A card entering an executable stage is automatically eligible; no second Start click is needed.
- Backlog has no automatic entry workflow.
- Untagged work uses the configured default agent.
- Tagged work runs only on a matching configuration. Initial tags can be agent names.
- A submitted @mention addresses that request's agent and starts a fresh conversation; it does not permanently retag the card.
- Use Herdr's existing runnable agents/commands/templates/configuration. Do not create a separate Canban-only provider support program.
- Show missing/ambiguous routing or unavailable startup configuration instead of pretending a launch succeeded.

Exact default inheritance and configuration fields can be refined during implementation. A simple Project-visible default and named routing configuration are sufficient starting points. Herdr core has no brand registry; the reusable launch setup supplies the actual command/profile.

## JSON configuration

Workflow/template customization is JSON-first. A UI editor and complex live migration are deferred.

Illustrative routing configuration:

```json
{
  "project_id": "project_123",
  "default_agent": "codex",
  "agents": {
    "codex": { "core_template": "canban-codex", "tags": ["codex"] },
    "claude": { "core_template": "canban-claude", "tags": ["claude"] }
  },
  "limits": { "per_project_active": 3 }
}
```

This is not hc configuration or a selected schema. Reuse the effective launch configuration and pass the task/context through existing core mechanisms. New logical requests start fresh provider conversations; reconnects refer to the previous execution.

## Completion is the agent's declaration

The agent explicitly answers/reports whether it has completed. Its completed report is enough to invoke the configured workflow callback. MVP does not independently judge output quality, require test/artifact verification, or wait for process exit before moving the card.

For the default development workflow:

```text
agent says completed
        |
record report + completion event
        |
template callback: Develop -> Review
        |
dispatch ordinary review-report task
```

The proposed concrete encoding is execution.report through the bundled Canban CLI and local IPC; its payload/receipt and launch environment are defined in [MVP interfaces](../interfaces/mvp-contract.md). This is a shared interface, not a verification subsystem. Summary/change/check information is useful context but does not become an independent progression gate. Human acceptance, merge, and archive remain distinct.

The completion callback belongs to the initiating request. A response to an @mention is recorded against its comment; it does not acquire the development callback merely because the profile or card matches. Process an identical accepted completion once in the normal path.

## MVP deliberately ignores residual old execution

The ordinary workflow advances after an agent reports completion. If an old process or mentioned agent exceptionally continues, do not build process-drain, quiescence, exclusive-writer handover, or orphan tracking before progression. This simplification is explicitly chosen by the user.

Capacity limits track active managed logical work; they are not a promise to constrain every surviving process or external core client. Logical completion can release the application's admission slot without requiring observed terminal exit. Keep runtime facts visible where available, but do not introduce a completion barrier around them.

Manual cancellation can use core's existing controls. Advanced coordination for moving active cards, conflicting writers, stale process activity, and descendant enforcement is deferred.

## Worktree handling

For Git work, preparation can create/reuse the card's execution worktree. Development, Review, and rework keep using it through the active card lifetime. A fresh Session does not require a new checkout.

Handle managed-worktree cleanup at Archive, not after every Session or stage. A non-Git Project can use its configured directory. Ordinary preparation failures should be reported rather than silently running in a different directory. Complex resource leases/handover are outside MVP.

## Blocked work and attention

If the agent needs input or reports blockage, produce the relevant application event, mark the card needs attention, and generate an internal notification event. The Board stage remains identifiable; no mandatory Blocked column is added.

The internal UI/notification subscriber makes the task discoverable. Native macOS notifications are later. See [events and notifications](events-and-notifications.md). A report of not completed/needs help does not invoke the completed callback.

## Minimal runtime bookkeeping

Persist enough request/attempt association and history to know which task was launched and which callback handles its report. Reuse core runtime identifiers and the proposed request/attempt logical key as described in [ID research](../research/herdr-identifiers.md).

Core terminal.run has no durable application idempotency key. Keep basic replay protection and show an unknown launch rather than automatically starting another on every timeout. This basic control is separate from the deliberately deferred old-process coordination. Detailed leases, generation fences, and fault-recovery protocols are not extra MVP product gates.

Core observations and reported status are useful input. Core status remains program-reported and may be unavailable; the common completion/blockage reporting interface must let existing launched agents express the product outcome. No provider-specific completion evaluator is required.

## Timed readiness

Scheduler owns queue/capacity/time readiness. A card's saved timing attributes belong to Kanban; initially implement clock waiting and due triggers as a timers module inside canban-scheduler. Workflow Engine decides a configured workflow action when a trigger fires; reminders can target Notifications. See [crates and timers](../architecture-crates-and-timers.md). This responsibility does not by itself promote advanced schedules into MVP.

## Triggers and effects

| Trigger | MVP behavior |
| --- | --- |
| Enter Backlog | Record event/history; no stage launch |
| Enter Develop / Review / custom executable stage | Schedule configured work in a fresh generic Session |
| Within-stage reorder | Update/history; no new stage entry |
| Submitted @mention | Schedule fresh conversation and associate response with comment |
| Agent declares completed | Invoke the request's configured completion callback |
| Agent blocked / needs input | Needs-attention card condition + internal notification event |
| Archive | Retain card history and handle managed-worktree cleanup |
| Description/tag/config update | Publish update; use explicit current/new request configuration, without launching on every keystroke |

All internal operations participate in the Rust enum/in-memory channel/topic-subscription direction. Detailed event architecture follows MVP. Scheduled-work features remain later product work rather than a reason to build that infrastructure now.

## Acceptance focus

Demonstrate automatic tag/default dispatch, fresh @mentions, self-reported completion driving the default Review report, blocked-card notification, JSON bindings, worktree retention until Archive, normal replay protection, and the shared agent interface. Do not require independent task verification or residual-process shutdown coordination to pass MVP.

Runtime facts are source-backed in [core API](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/schema/README.md); the chosen simplified product behavior is Canban policy.
