# Agent CLI and extension boundary

Status: implemented 2026-11-07; current lifecycle/API description reconciled 2026-11-10. [Profile schema](agent-profiles.md) and [MVP acceptance](../mvp-acceptance.md) are the current references. The earlier provider-specific Rust adapters and mandatory notify/Stop callbacks were removed.

## CLI capability surface

The CLI and native UI call the same AppServer operations through a same-user socket. Remote Agents use the [Core-backed CLI bridge](../multi-machine.md#remote-context-and-agent-cli) to that same authority, not a second product server. Dedicated helpers are `report`, `reply`, `bind-session`, `status` and `snapshot`; the general form is `canban call METHOD JSON`. `serve` hosts the same AppServer without the UI.

| Area | Operations |
| --- | --- |
| Discover | app.ping/snapshot, project.list, card.get/list, timeline.list, execution.get/output |
| Manage | project.create, card.create/update/move/context.update/attention.clear/archive/restore |
| Context | note.save, comment.add with mentions, attachment.add, resource.read/materialize |
| Outcomes | execution.report; agent.reply; CLI report --state working/completed/blocked/failed and reply --to QUESTION_ID --file PATH |
| Agent control | agent.open/bind/interrupt/end, execution.retry |
| Code delivery | review.preview, review.merge |
| Configuration | config.get/reload, profile.presets, herdr.status/templates |

`agent.turn_ended` is a deprecated ignored compatibility operation; hidden `agent-notify` is a no-op for old invocations. Neither closes or binds a conversation. No lifecycle callback is required by the new profile interface.

Launch supplies `CANBAN_CLI`, `CANBAN_HOME`, `CANBAN_ENDPOINT`, `CANBAN_REQUEST_ID`, `CANBAN_ATTEMPT`, `CANBAN_PROFILE_ID`, `CANBAN_SESSION_ID` and `CANBAN_RUN_DIR`. Use the current invocation binding rather than a command remembered from older history. Reports/replies belong in the run directory, outside delivered source. The CLI sends file bytes to AppServer. Mutations use `request_key` for retry receipts; Card edits also use revision checks. See [Review delivery](review-delivery.md) for concrete delivery payloads and uncertain-result handling.

The socket has application-wide authority, including other Cards and Agent controls. There is no per-Agent capability sandbox in Canban. Provider sandbox/approval settings remain active. Terminal input/stream traffic goes directly through Herdr rather than a business operation per keystroke.

## Multi-Agent architecture

```text
Workflow JSON: stage/task/transition/profile --+
                                             v
Profile + preset defaults/overrides -> Scheduler / AppServer
                                             |
Card + profile ID -> Conversation -> Request -> Attempt
                    opaque history  task       core ID + terminal
                                                   |
                                             Herdr -> Agent PTY
                                                   |
                     Canban CLI <--- report / reply / bind
```

The global Agent budget defaults to eight and is configurable; there are no additional Board/profile caps. Human gates pause automatic requests. Separate Cards have separate conversations/worktrees; profiles on one Card share its worktree. A live Card/profile is reused. Different profiles, including two using Codex, have independent provider histories. This is the selected simple model, without writer leases or a generalized Agent-team protocol.

A live mention is additional input and a durable Question. Its explicit reply is independent of completing the original assignment. Stage entry queues work with its own completion callback. An Agent can mention another configured profile through comment.add, using ordinary scheduling; Canban does not automatically orchestrate that CLI's internal subagents.

## What arbitrary commands must supply

| Capability | Requirement |
| --- | --- |
| Launch and interact | A Herdr-runnable interactive command |
| Advance workflow | Explicit Canban report with current request/attempt |
| Durable question answer | Canban reply with Question ID |
| Exact history Resume | Known/preallocated provider ID, bind-session where needed, resume_argv |
| Automatic idle exit | Verified idle/done, exact resumable history, configured exit action, elapsed inactivity timeout and no protected pending work |

Commands can run without every optional capability. A command without Resume uses explicit fresh context. Completion keeps the live Terminal available. A command without usable status stays available for manual exit; completion itself does not send an exit command. See [Terminal interaction](../terminal-interaction.md). Herdr status does not prove business completion or provider-history durability; real-provider acceptance establishes the latter boundary.

Presets are editable configuration defaults, not provider hook registrations. Rust expands commands and consumes the common contracts. Optional wrappers supply provider-specific ID acquisition or command syntax when necessary; they must not duplicate sufficient Herdr lifecycle observations. There is no plugin/SDK requirement. Workflow templates select profiles and stage work; Herdr launch templates do not provide universal history Resume and are not an alternate runtime mode.

## Manual Terminal API

The same AppServer boundary supports `terminal.create` with optional `machine`, `cwd`, `inherit_terminal_id`, `project_id` or `card_id` launch context and a `request_key`; a Project is not required. It also supports `terminal.rename` (`terminal_id`, `name`), `terminal.context`, `terminal.interrupt`, `terminal.end` and the unified `terminal.close` lifecycle (`terminal_id`, `request_key`). These are available through `canban call METHOD JSON`; transport never bypasses the owning Core identity. Create/control request keys prevent effect replay after uncertain receipts. A manual Shell does not become a Workflow Agent merely because an Agent command is run inside it. Tab selection and leaving the viewer are navigation only. Tab X, Command+W and Close terminal instead invoke `terminal.close`: AppServer resolves and terminates the exact live runtime, and UI removes the tab on a successful receipt. Failed/uncertain closure remains reachable. History and explicit exact Resume survive closure.
