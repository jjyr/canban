# Conversations and unified Agent terminals

Status: implemented; current contract reconciled 2026-11-14 (SQLite schema 15, desktop API 34). All terminals—manual Shells, manual ephemeral CLI Agents, and scheduled/Workflow Card-role Agents—share `TerminalEntry`. Separate `Request` and `Attempt` records have been removed: each `(card_id, profile_id)` role conversation reuses one durable `TerminalEntry` (`id = "{card_id}:{profile_id}"`) in place across continuations, retries and resumes via `TerminalEntry.agent: Option<AgentRun>` (plus `pending: Option<QueuedAssignment>`). Scheduling triggers differ; there are no question/stage Session classes. See [Agent interaction](agent-interaction.md) for product entry points and [Terminal interaction](../terminal-interaction.md) for inactivity/navigation.

## Identity and ownership

```text
Project -> Card -> profile conversation (Card ID + profile ID)
                     |       provider + exact provider_session_id
                     |
                     +-> TerminalEntry (`{card_id}:{profile_id}`)
                           +-> AgentRun (active assignment + workflow_run_id/workflow_step + number)
                                 immutable machine / cwd / workspace
                                 Core identity + qualified terminal address
                                 captured input / replies / report / output
                                 optional pending QueuedAssignment
```

The conversation is Canban's durable Card/profile association with an opaque provider history ID, paired 1:1 with the role's `TerminalEntry` (`{card_id}:{profile_id}`). `AgentRun` records the current or last assignment on that terminal and queues at most one follow-up `QueuedAssignment` while live. Neither the provider history ID nor the Herdr terminal address replaces these product identities. Developer/Reviewer profiles remain separate conversations and `TerminalEntry` rows even if both use Codex. Different Cards never share the same conversation binding.

Accepted [workflow model](workflow-engine.md): `WorkflowRun` and step executions track durable process progress and link directly to the active role terminal via `terminal_id` (`{card_id}:{profile_id}`). Project Workflows configuration conversations have Project + purpose + Profile ownership, independent of business Cards. They reuse Profile commands, Scheduler admission and the native Terminal. They cannot approve a business human gate. Onboarding remains separate; see [Edit with agent](workflow-engine.md#edit-with-agent).

## Independent state dimensions

| Dimension | Meaning | It does not establish |
| --- | --- | --- |
| Workflow stage | Backlog / Develop / Review, or a configured stage | Whether an Agent is running or finished |
| Assignment outcome | Explicit Canban working/completed/blocked/failed report, or recorded execution failure | Whether the PTY is idle, connected or exited |
| Runtime state | Herdr working/blocked/idle/done/finished observations | Task completion or permission to advance workflow |
| Connection | Connected, connecting, disconnected or uncertain observation | A confirmed process exit |
| Human gate | Saved task decision or required Review | An unread-notice count or a runtime permission dialog |

These dimensions can coexist: a Card in Review may have a completed assignment, a live idle Terminal and a pending human gate. Terminal **Inactive** is a live-session visibility policy; it is not an outcome or a new workflow stage. A confirmed unreported exit becomes Failed history and clears its runtime wait, without synthesizing a task gate. Explicit blocked/failed reports and Review gates retain their independent human decisions. See [attention handling](needs-attention.md).

## Assignments and live reuse

A live @profile comment becomes a durable Question associated with its comment and active `TerminalEntry` (`terminal_id`). It is sent immediately through Herdr. `canban reply` records the answer and Timeline event without completing the original assignment. An absent/exited profile starts/resumes through the same Scheduler; its follow-up assignment has no stage callback. Delivery uncertainty is retained rather than blindly resent.

Stage/Workflow entry queues an assignment onto the Card/profile `TerminalEntry` (`AgentRun`, or `pending: QueuedAssignment` when a turn is already active) with the step's task, `workflow_run_id` / `workflow_step` and completion callback. A completed, connected idle/done invocation in the same environment can accept a new assignment when the saved command configuration is compatible and no question is outstanding, promoting `pending` in place on the same `TerminalEntry` while retaining the physical PTY and advancing `AgentRun.number`. Otherwise the same Card/profile waits for a usable boundary rather than starting a duplicate process.

Other Cards/profiles run concurrently within the configurable global budget, default eight. Starting/working/closing and uncertain invocations reserve slots; verified blocked/idle/done/error states release execution capacity without destroying their Terminals. Queued work consumes no slot, and manual Shells are outside the Agent budget. There are no additional Board/profile caps. [Scheduler](scheduler.md) owns admission, later-stage priority and human gates.

## Completion, exit and controls

```text
explicit completed report -> save outcome + callback once, or wait at human gate
                          -> keep live Terminal available
                          -> verified idle/done + no pending work + idle timeout
                          -> configured normal exit -> observe original PTY exited
                          -> remove Terminal row/tab; retain exact provider history ID

human input before exit -> renew activity -> later idle/done can become Inactive again
```

A report alone cannot establish idle. Turn-end, idle, a reply and process exit cannot establish business completion. Completed live invocations accept later questions without repeating their callback. Idle reclamation requires an exact resumable provider binding and configured normal exit; blocked, unknown/disconnected, unfinished automatic work and unanswered questions remain protected. Merely viewing a Terminal does not prevent reclamation indefinitely.

Interrupt targets the live PTY. X, Command+W and Close terminal dispatch the same AppServer `terminal.close` operation, which resolves the exact runtime, records intent and terminates it before confirming tab removal. Failure or uncertain termination keeps the tab reachable. Leaving the page, hiding the list or quitting the UI only detaches the viewer. Core identity is checked before input/control/exit; old close receipts cannot terminate a resumed invocation.

Selecting a queued/history row or traversing Back/Forward never launches work. Explicit Card/Overview Resume queues a resumed `AgentRun` on the same `TerminalEntry` using the exact saved provider ID and original execution environment, subject to Scheduler admission. Missing history is visible; starting fresh with retained Card context is a separate explicit recovery action.

## Persistence and recovery

SQLite WAL stores `Conversation`, `Question` and `TerminalEntry` (`terminals` table, including `AgentRun`) metadata. Long captured context, replies, reports and retained terminal output are Card-owned files. Historical final-response snapshots remain readable; the lifecycle does not capture provider callback payloads. Archive compresses these files and record exports but does not back up provider-owned histories. Restore is required before invoking an archived Card; a removed clean managed worktree can be recreated from its retained Card ref.

AppServer restart never blindly replays a launch without its receipt, an uncertain question delivery or a physical terminal control intent. These remain unknown/recovery for inspection. Runtime subscription reconnects rebuild observations without changing product identities. [Multi-machine execution](../multi-machine.md) owns environment placement; [runtime observation](../runtime-observation.md) owns connection/exit classification. Provider acceptance limits remain in [implementation](../implementation.md).

## Terminal entries and opened tabs

[TerminalEntry](../terminal-workspace.md) unifies manual Shells (`runtime`), manual ephemeral CLI Agents (`runtime` + `agent_engine`), and scheduled/Workflow Card-role Agents (`agent: Option<AgentRun>`, `id = "{card_id}:{profile_id}"`). A manually opened Shell has its own entry and owning Core/PTY, no required Project, and only directory-derived Project/Card hints. Manual Shells and ephemeral CLI Agents never manufacture scheduled `AgentRun` assignments or advance Workflow automatically. Launch cwd is an argument; live cwd comes from Herdr rather than a second durable Shell directory field.

Agent entries use Card/stage/Provider/Profile titles plus machine; independent Terminals use a separate monotonic Terminal N sequence plus machine. Both support custom names and one opened tab each. Finished entries leave the Terminal list/tabs, while saved Agent history remains accessible through Card/Overview. Shell Reopen creates a new PTY/entry; Agent Resume retains its conversation. Opened tabs/selection are process-local UI state; durable names and bindings survive restart.
