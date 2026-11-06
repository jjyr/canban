# MVP scope and implementation sequence

Status: scope derived from the user's 2026-11-05/06 decisions; the first native MVP is now implemented. [Implementation and acceptance](implementation.md) records supported behavior, real-Herdr/Codex verification, native interaction checks and remaining limits. The acceptance goals below are the product plan rather than proof that every possible scenario has been validated.

## The first-release loop

```text
Project + directory -> local Card in Backlog
                              |
                           Develop
                              |
                  Canban automatically dispatches
                  one generic fresh Session via core
                              |
                  agent declares completed
                              |
                  template advances to Review
                              |
                  fresh Session produces ordinary report
                              |
                  human disposition -> Archive
                                         |
                                handle managed worktree cleanup

Submitted @agent comment -> the same Scheduler/Session mechanism
                        -> response recorded on the card
```

The stage workflow and its completion callback decide progression. Agent self-report is the completion signal; MVP adds no independent test, artifact-quality, or process-exit gate before advancing. Human review/acceptance and merge remain separate from that report.

## Included

- Directory-backed Projects, one Board each, local cards, project-local numbers, descriptions/tags/resources/image paste, basic Notes edit/preview and selected context.
- Backlog/Develop/Review default, required no-op Backlog, automatic stage Scheduler and fresh @mention conversations through one Session model.
- Tag/default routing and independent concurrent Sessions without application concurrency caps.
- Agent completion/progress/blocking reports, unified card timeline, comments/results, and a minimal agent-callable application interface.
- Herdr's existing runnable agent setup through core; no new provider-integration program or Canban-only agent family restriction.
- Development checkout/worktree preparation where applicable, reuse through review/rework, and worktree handling at Archive.
- Plain review reports. Shear UI follows later.
- JSON-defined workflow/template configuration. No template-customization UI in MVP.
- A needs-attention card condition and an internal notification event for blocked/input-needed work.
- A minimal event-driven Rust application: enum events, in-memory channel, and interested components subscribing by topic. Detailed event architecture follows MVP.
- Native Rust/NSH main UI with optional embedded WebView, and an NSH-independent AppServer co-hosted in the desktop process. A separately deployed Canban daemon is not a first-release requirement.
- A separate canban-ui-kit, suitable NSH behavior reuse, component contracts, and a native gallery using production components. Develop shared controls and product composition through a useful vertical slice; follow the approved [UI architecture](ui-architecture.md) and regression conventions.

## Explicit simplifications

| Decision | MVP behavior |
| --- | --- |
| Completed? | Let the agent explicitly answer/report completed, then follow the configured workflow |
| When to move | Normal progression happens after that report, not while waiting for independent execution verification |
| Old process still running | Do not add process-drain, quiescence, exclusive-writer handover, or orphan coordination as progression gates |
| Worktree lifetime | Keep through Develop/Review/rework; handle cleanup at Archive |
| Review depth | Ordinary report, without the Shear editor |
| Templates | JSON configuration; defer rich editing and complex live migration |
| Agent support | Reuse Herdr's existing launchable agents/configuration; core remains the execution runtime |
| Blocking | Mark card needs attention and generate an internal notification event |
| Notifications | Internal first; native macOS delivery later |
| Event design | Minimal enum/channel/topic subscriptions now; detailed architecture after MVP |

Residual executions, including mentioned agents that continue running, are a consciously accepted MVP limitation. Logical completion need not wait for terminal exit. Basic replay protection for a repeated completion or submitted operation is still useful; it must not grow into the deferred coordination system.

## Event-driven direction

```text
UI/API actions + agent reports + runtime observations
                         |
                   Rust AppEvent enum
                         |
                  in-memory channel
                         |
                 topic subscriptions
              /       |       |        \
         workflow   card/UI  timeline  internal notifications
         dispatch
```

The in-memory channel transports events; persisted card history remains the timeline's responsibility. This direction does not require all internal events to become a durable event store. No channel crate, full enum, topic taxonomy, delivery/replay guarantees, or subscription framework is selected here. See [events and notifications](product/events-and-notifications.md).

## Build sequence

1. **Runtime/UI seam:** connect to a scratch core, launch a harmless fixture through an existing launch path, read output/status, and verify native Rust/NSH command/state updates against the co-hosted AppServer, with WebView only for a specific optional surface. Do not touch the user's live core.
2. **Project and card:** directory/Board/card numbers, description/resources, basic Notes, comments, archive, and recoverable timeline data.
3. **Minimum events and interface:** route application actions through the small shared event direction and expose core card/comment/report operations to agents. Do not implement the deferred event framework.
4. **Automatic Scheduler:** JSON workflow bindings, default/tag selection, persisted launch queue without concurrency caps, generic Session launch, and self-reported completion callbacks.
5. **Complete the loop:** ordinary Review report, needs-attention/internal notification, fresh mentions, worktree reuse and archive handling, human disposition, and restart/reconnect behavior.

The [architecture](architecture.md) defines the co-hosted logical service and future daemon seam. [Agent interface research](research/agent-interface-options.md) recommends thin CLI plus local RPC and shared typed handlers; MCP can be a later adapter. These transport recommendations are not a second application authority.

The [identifier research](research/herdr-identifiers.md) recommends reusing core identity and terminal address for runtime binding and request ID plus attempt number for logical execution. This requires no extra Session category.

## Release acceptance

- Create a Project in an existing or new directory without requiring Git initialization.
- Create a numbered local card, selected context, comment, and pasted image; retain them after restart.
- Move an untagged card into Develop and automatically use the default agent; tagged work uses its matching configuration.
- Record the agent's completed report and execute the default Review transition once without waiting for its terminal to exit or independently judging its work.
- Produce an ordinary Review report through the existing generic execution path.
- Submit an @mention, start a fresh conversation, and attach its response to the initiating comment.
- Mark a blocked card needs attention, record the event/history, and expose the internal notification.
- Load a JSON template and use its stage/agent/completion bindings; no visual designer is required.
- Reuse the task worktree across development, review, and rework; do not clean it before Archive.
- On Archive, apply the defined managed-worktree handling without treating card archive as deletion of the Project directory or history.
- Use SQLite with WAL. On Card archive, compress related Card/Agent execution data and results with zstd/lz4, retaining readable history and an archive lookup record. Verify safe packaging/retry and on-demand access; detailed container/codec selection follows the [data architecture](data-architecture.md#compressed-card-archives).
- Replay accepted moves/results/comments without duplicate normal workflow effects; reconnect does not itself create fresh work.
- Use the agent interface and UI with equivalent card/timeline outcomes.
- Describe actual behavior when the window/coordinator is closed; do not claim scheduling continues if its owner is absent.

These are acceptance requirements, not executed tests. Provider startup/completion reporting and native behavior still need implementation validation; source inspection alone is insufficient.

## Deferred

GitHub issue backend and synchronization, Shear review UI, macOS notification delivery, template GUI/designer, complex active-template migration, detailed event infrastructure, old-execution/orphan coordination, automated verification gates, provider-specific integration expansion, advanced schedules, and merge automation.

Some details can be refined during implementation without reopening the product model: exact JSON fields, completion-report encoding, attachment limits, notification display, and archive cleanup treatment of modified managed worktrees. Keep the agreed simplifications intact.

The user subsequently requested concrete interface mechanisms before implementation. The [MVP interface contract](interfaces/mvp-contract.md) now supplies a proposed report CLI/local IPC mechanism, execution context, payload/receipt, and failure behavior. Review this critical path before coding it. Topic architecture and a complete interface catalog remain deferred rather than blocking the useful MVP loop.
