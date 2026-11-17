# Agent CLI and extension boundary

Status: implemented 2026-11-07; current lifecycle/API description reconciled 2026-11-10. [Profile schema](agent-profiles.md) and [MVP acceptance](../mvp-acceptance.md) are the current references. The earlier provider lifecycle adapters and mandatory notify/Stop callbacks were removed. Per-invocation command/environment adaptation now lives in canban-profiles; see [launch adaptation](agent-profiles.md#per-invocation-launch-adaptation). It does not interpret provider lifecycle or install hooks.

RPC and CLI use [Agent-first response defaults](../agent-response-contract.md):
concise summaries and write receipts, explicit fields/full content requests,
server-owned selection/pagination, and formatting-only `--human`. The default
response change requires client migration; no legacy raw-response mode is kept.

## CLI capability surface

The CLI and native UI call the same AppServer operations through a same-user socket. Remote Agents use the [Core-backed CLI bridge](../multi-machine.md#remote-context-and-agent-cli) to that same authority, not a second product server. Dedicated helpers are `report`, `reply`, `card update`, `card adopt`, `bind-session`, `skill` (`--skill`), `status` and `snapshot`; the general form is `canban call METHOD JSON`. `serve` hosts the same AppServer without the UI.

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

Launch supplies `CANBAN_CLI`, `CANBAN_HOME`, `CANBAN_ENDPOINT`, `CANBAN_PROJECT`, `CANBAN_CARD` (short Card number), `CARD_ID` (internal Card ID), `CANBAN_PROFILE_ID`, `CANBAN_SESSION_ID` and `CANBAN_RUN_DIR` across both scheduled Agent terminals and Card-opened terminals. Reports/replies belong in the run or Card scratch directory, outside delivered source. The CLI sends file bytes to AppServer. Mutations use `request_key` for retry receipts; human Card edits also use revision checks, while Agent and Card-terminal CLI commands (`card show`, `card update`, `card adopt`, `report`, `reply`, `bind-session`) resolve the target Card via `CANBAN_CARD` + `CANBAN_PROJECT` (or working-directory inference). See [Review delivery](review-delivery.md) for concrete delivery payloads and uncertain-result handling.

## Built-in Canban CLI Skill

Canban bundles a version-locked Agent Skill at [`skills/canban/SKILL.md`](../../skills/canban/SKILL.md) teaching coding agents how to operate Cards, comments, reviews, workflows, and execution outcomes through `$CANBAN_CLI` without mutating user-global agent folders automatically:

- **Local materialization**: `AppServer::open` atomically writes `<CANBAN_HOME>/skills/canban/SKILL.md` (and `stage_remote` stages it onto remote execution homes), and `canban skill` / `canban --skill` prints the embedded Markdown directly without opening application data.
- **CLI discovery**: `canban --help` and `canban skill --help` tell Agents to read the bundled instructions, or create `canban/SKILL.md` in their own skill discovery directory and write the output of `canban skill` into it. The Agent chooses its appropriate directory; Canban does not implement provider-specific installation commands. The existing optional `npx skills` UI guidance remains available.
- **Optional user-guided installation**: Both the Agent Setup onboarding wizard (`agent-setup`) and `Settings -> Agents & Identities` display an optional **Canban CLI Skill** block right below **Default Agent** with `npx skills add <CANBAN_HOME>/skills --skill canban -g`, a **Copy** button (`copy-skill-command` in the Command Catalog), and an **Open Terminal (`⌘T`)** action.
- **Global `⌘T` standalone Terminal**: `new-terminal` (`⌘T`) is bound globally so users can open a standalone Shell tab unassociated with any Card directly from Settings, Agent Setup, Board, or Terminals (while preserving Card-scoped terminal creation when a Card's Live Terminal split is open).

The socket has application-wide authority, including other Cards and Agent controls. There is no per-Agent capability sandbox in Canban. Provider sandbox/approval settings remain active. Terminal input/stream traffic goes directly through Herdr rather than a business operation per keystroke.

## Multi-Agent architecture

```text
Workflow YAML: stage/step/transition/profile --+
                                             v
Profile + preset defaults/overrides -> Scheduler / AppServer
                                             |
Card + profile ID -> Conversation + TerminalEntry (`{card_id}:{profile_id}`)
                     opaque history  AgentRun (task + core ID + terminal + pending)
                                                   |
                                             Herdr -> Agent PTY
                                                   |
                     Canban CLI <--- report / reply / bind
```

The global Agent budget defaults to eight and is configurable; there are no additional Board/profile caps. Human gates pause automatic requests. Separate Cards have separate conversations/worktrees; profiles on one Card share its worktree. A live Card/profile is reused. Different profiles, including two using Codex, have independent provider histories. This is the selected simple model, without writer leases or a generalized Agent-team protocol.

A live mention against an already running non-comment task is additional input and a durable Question; its explicit `canban reply` is independent of completing the original assignment. By contrast, when a mention creates a dedicated `RequestTrigger::Comment` execution (`comment:<event_id>`), its prompt (`prompts/context/reporting_comment.txt`) instructs the Agent to answer and complete the assignment in a single `canban report` call without rendering a separate `canban reply` section (and if an Agent ever calls both, the follow-up `completed` report is marked `comment_replied: true` and hidden from Timeline/unread counts). Built-in context, runtime, workflow-step and profile role prompt templates live as plain-text `.txt` files under the top-level `prompts/` directory and are embedded at compile time via `include_str!`. Stage/workflow entry queues work with its own completion callback (`RequestTrigger::Workflow` / `RequestTrigger::StageEntry`), and opening an interactive discussion uses `RequestTrigger::Interactive`. An Agent can mention another configured profile through `comment.add`, using ordinary scheduling; Canban does not automatically orchestrate that CLI's internal subagents.

## What arbitrary commands must supply

| Capability | Requirement |
| --- | --- |
| Launch and interact | A Herdr-runnable interactive command |
| Advance workflow | Explicit Canban report from the active Card/profile Agent terminal (`CANBAN_CARD` + `CANBAN_PROFILE_ID`) |
| Durable question answer | Canban reply with Question ID |
| Exact history Resume | Known/preallocated provider ID, bind-session where needed, resume_argv |
| Automatic idle exit | Verified idle/done, exact resumable history, configured exit action, elapsed inactivity timeout and no protected pending work |

Commands can run without every optional capability. A command without Resume uses explicit fresh context. Completion keeps the live Terminal available. A command without usable status stays available for manual exit; completion itself does not send an exit command. See [Terminal interaction](../terminal-interaction.md). Herdr status does not prove business completion or provider-history durability; real-provider acceptance establishes the latter boundary.

Presets are editable configuration defaults, not provider hook registrations. Rust expands commands and consumes the common contracts. Optional wrappers supply provider-specific ID acquisition or command syntax when necessary; they must not duplicate sufficient Herdr lifecycle observations. There is no plugin/SDK requirement. Workflow templates select profiles and stage work; Herdr launch templates do not provide universal history Resume and are not an alternate runtime mode.

## Manual Terminal API

The same AppServer boundary supports `terminal.create` with optional `machine`, `cwd`, `inherit_terminal_id`, `project_id` or `card_id` launch context and a `request_key`; a Project is not required. It also supports `terminal.rename` (`terminal_id`, `name`), `terminal.context`, `terminal.interrupt`, `terminal.end` and the unified `terminal.close` lifecycle (`terminal_id`, `request_key`). These are available through `canban call METHOD JSON`; transport never bypasses the owning Core identity. Create/control request keys prevent effect replay after uncertain receipts. A manual Shell does not become a Workflow Agent merely because an Agent command is run inside it. Tab selection and leaving the viewer are navigation only. Tab X, Command+W and Close terminal instead invoke `terminal.close`: AppServer resolves and terminates the exact live runtime, and UI removes the tab on a successful receipt. Failed/uncertain closure remains reachable. History and explicit exact Resume survive closure.
