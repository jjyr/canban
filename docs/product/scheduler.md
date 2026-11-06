# Scheduler and agent-reported progression

Status: user-directed MVP behavior, updated 2026-11-06. Basic routing, unrestricted concurrent Sessions and explicit completion callbacks are implemented. Current configuration and concurrency evidence are described below; advanced mechanisms remain deferred.

## Next interaction direction versus the current runtime

The user has accepted Card/profile conversation reuse and separate configuration identities for distinct contexts such as Developer and Reviewer. The [Agent interaction design](agent-interaction.md) is authoritative for that target and records the proposed independent profile registry plus template references. Card/profile conversation reuse, stage-profile references, role instructions, interactive launch/resume and immediate active-mention delivery are implemented. The current lifecycle is described in [Sessions](sessions.md); earlier fresh-conversation wording below is historical.

## One scheduling path

```text
stage entry / submitted @mention / configured action
                         |
                  application event
                         |
                   Canban Scheduler
          task + profile + context + callbacks
                         |
                 persisted launch queue
                         |
                 new invocation / same profile history
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
- An automatic stage with a profile reference selects that global profile.
- A stage without a reference uses Card tags, then the configured default Agent for untagged work.
- Tagged fallback work runs only on a matching configuration. Initial tags can be Agent names; tags do not override a named stage profile.
- A submitted @mention addresses the Card/profile conversation: append immediately while live, otherwise start/resume. It does not permanently retag the Card.
- Use Herdr's existing runnable agents/commands/templates/configuration. Do not create a separate Canban-only provider support program.
- Show missing/ambiguous routing or unavailable startup configuration instead of pretending a launch succeeded.

The current default and profile registry are application-wide; stage references, explicit mentions and legacy tags select their entries. See [profile configuration](agent-profiles.md). Per-Project default overrides are not implemented. Herdr core has no brand registry; the configured existing command/template supplies the launch setup.

## Cross-Board concurrency and isolation

One Project currently owns one Board. Boards share one Scheduler, with no global, Project or per-profile concurrency limit. Queued requests are launched in order without waiting for other Sessions to complete; launches are currently issued one at a time by the Scheduler loop, while the resulting Agents run concurrently. The queue records pending work, rather than reserving capacity. Legacy capacity fields in configuration and saved requests are ignored. Limits can be introduced later if there is a demonstrated need. Routing does not automatically switch provider or assign different brands to different Boards.

Each request/attempt, context path, report and runtime binding is scoped to its Card/Project. Git Cards get separate worktrees; stages of the same Card reuse one worktree. Non-Git/unborn Projects use the configured directory directly, so choosing the same directory for multiple Projects does not create filesystem isolation. Shared external resources and same-user process permissions remain shared. Progression has no residual-process drain gate.

A [real two-Board Codex acceptance](../real-agent-acceptance.md) observed simultaneous development and simultaneous review, then archived both Cards and checked report readability and cross-Board associations. Cross-provider live behavior and exclusive security/resource isolation are outside that evidence.

## JSON configuration

Workflow/template customization is JSON-first. A UI editor and complex live migration are deferred.

Example application configuration (preserve other profiles when editing):

```json
{
  "hc": "hc",
  "hc_session": "default",
  "hc_env": {},
  "default_agent": "codex",
  "profiles": [
    {
      "id": "codex",
      "argv": ["codex", "--no-daemon", "--sandbox", "workspace-write", "--add-dir", "{home}", "{prompt}"],
      "template": null,
      "tags": ["codex"]
    }
  ]
}
```

This is Canban's config/agents.json schema, not hc configuration. Reload via Settings or config.reload and queue new work; queued requests retain their saved profile snapshot. New invocations reuse the Card/profile provider ID; reconnects refer to the existing terminal.

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

There are no application concurrency limits. Logical completion does not require observed terminal exit. Keep runtime facts visible where available, but do not introduce a completion barrier around them.

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

Scheduler owns queue/time readiness. A card's saved timing attributes belong to Kanban; initially implement clock waiting and due triggers as a timers module inside canban-scheduler. Workflow Engine decides a configured workflow action when a trigger fires; reminders can target Notifications. See [crates and timers](../architecture-crates-and-timers.md). This responsibility does not by itself promote advanced schedules into MVP.

## Triggers and effects

| Trigger | MVP behavior |
| --- | --- |
| Enter Backlog | Record event/history; no stage launch |
| Enter Develop / Review / custom executable stage | Schedule configured work in a new invocation / same profile history |
| Within-stage reorder | Update/history; no new stage entry |
| Submitted @mention | Append/start/resume addressed profile; associate reply with comment |
| Agent declares completed | Invoke the request's configured completion callback |
| Agent blocked / needs input | Needs-attention card condition + internal notification event |
| Archive | Retain card history and handle managed-worktree cleanup |
| Description/tag/config update | Publish update; use explicit current/new request configuration, without launching on every keystroke |

All internal operations participate in the Rust enum/in-memory channel/topic-subscription direction. Detailed event architecture follows MVP. Scheduled-work features remain later product work rather than a reason to build that infrastructure now.

## Acceptance focus

Demonstrate automatic tag/default dispatch, fresh @mentions, self-reported completion driving the default Review report, blocked-card notification, JSON bindings, worktree retention until Archive, normal replay protection, and the shared agent interface. Do not require independent task verification or residual-process shutdown coordination to pass MVP.

Runtime facts are source-backed in [core API](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/schema/README.md); the chosen simplified product behavior is Canban policy.
