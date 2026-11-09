# Attention handling inside Overview

Status: implemented 2026-11-07 inside the global [Overview](overview.md). See [implementation and validation](../agent-overview-and-lifecycle.md) for current contracts and evidence. macOS notifications remain deferred.

## Current implementation

Canban blocked/failed reports persist task attention and a human gate. Herdr root blocked states persist independent execution-wait episodes and deduplicated Notices/Timeline events. Overview combines both sources with review/recovery context across Projects; its badge counts actionable rows, independently of unread notices and retained terminal count. Runtime recovery never approves a task gate. Status-aware capacity and resumable idle closure use the same original Core/terminal bindings.

## Terminology

Use **Needs attention** for the highlighted group/filter within Overview. It contains current user actions rather than every application event or an unread-message feed. Overview also includes Ready for review, Working, Queued, Idle and Closed.

| User-facing name | Meaning | Source | Primary action |
| --- | --- | --- | --- |
| Execution waiting | A running Agent needs input to proceed | Herdr root `blocked`, with permission/question/auth subtype when reported | Open terminal |
| Task needs decision | The Card assignment needs a human decision or intervention | Canban blocked report/human gate | Open Card |
| Review requested | A stage report/result needs human review before progression/delivery | Workflow `require_human_review` and saved result | Review changes / Open Card |
| Recovery needed | Startup, Git preparation, connection or execution recovery needs inspection | Existing Canban attention/error state | Inspect Card / Open terminal when available |

Subtypes give the concrete label: **Permission required**, **Sign-in required**, or **Answer required**. Preserve the raw `blocked` status/report in diagnostic/history data; separating product names does not require changing Herdr's protocol or adding Session classes. If runtime `kind` is missing, use **Execution waiting** rather than guessing permission from terminal text.

Task and execution waits remain independent. User input can resume the current Agent without accepting its Card result. Herdr returning to working cannot approve a Canban human gate. Conversely, approving a workflow does not answer a terminal permission dialog.

## Global entry, list and context

The global sidebar entry is **Overview**, above Projects, with a persistent count of actionable rows across Projects (including human Review). It replaces the existing Agent activity entry and the proposed separate Needs attention page. Visiting it does not approve a gate. Board remains the normal startup/workflow destination; its local attention filter remains a shortcut.

Use the grouped full-Agent list and collapsible context inspector in [Overview](overview.md). The attention group leads, followed by actual Review requests, Working, Queued and Idle; Closed is collapsed. No selection opens/resumes an Agent until the user invokes the appropriate action. Narrow context has a clear Back to Overview control, with selection/scroll/filters retained.

Each list item makes the following visible without opening another Project:

- Concrete action/reason and its source category.
- Project name, Card number/title, current stage and Agent profile/provider display name when known.
- A short reported summary and waiting time.

The selected context includes Card scope, the initiating stage/@question assignment, the actual Agent invocation and relevant report/comment links. Stage-triggered and mention-triggered assignments differ: a mention should show the actual question, not imply that it is implementing the whole Card. Use the saved request instruction/comment and captured scope to explain what that invocation was asked to do; do not treat the current mutable Card description as its exact historical input.

A Card-only recovery issue may have no Agent or terminal yet. Show that honestly, with the failure reason and Card action; do not manufacture an Agent identity. Raw request/attempt/Core IDs belong in expandable diagnostics.

For narrow widths, show the list and then selected context as separate surfaces with **Back to Overview**. Preserve list selection, scroll and filters.

## Herdr context boundary

The inspected core 0.1.18 Status schema exposes `state`, optional `kind`, `progress`, `app`, `title` and `msg`. `title`/`msg` are program-supplied context and may be empty. Core does not guarantee a structured permission command, a full approval request or a semantic chat message. `status_ms` records the last root state-word change; changes to the message/kind while still blocked must not be missed because the timestamp stayed the same.

Render the reported title/message with an explicit source label. When insufficient, load a bounded read-only terminal screen for the selected item through the original Core/terminal binding. Label its freshness/connection state. Read it on selection/refresh instead of reading every Agent screen for the list. This is terminal context, not an extracted Canban answer or permission schema.

Do not infer exact permission choices from output or offer generic Approve/Reject buttons over an unstructured screen. Open the actual terminal for interaction. Previewing must not focus the terminal, send keys or grant a permission. Existing native Terminal remains the interactive surface.

On lost/replaced Core or missing terminal, retain the last known context as unavailable/unverified. Do not send input to a reused address or assume the earlier wait was resolved. Show recovery actions appropriate to the actual binding/history.

Primary source: [Herdr status schema](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/status.rs#L13). These are inspected source fields, not fresh live permission-dialog acceptance.

## Handling and navigation

### Execution waiting

1. Select the item to see Project, Card, assignment, Agent and reported/terminal context.
2. Choose **Open terminal**. Attach the exact live invocation; do not start another Agent or resume an unrelated conversation.
3. Respond in the native terminal. Preserve the underlying provider's approval/authentication behavior.
4. When a trustworthy status leaves blocked for working/idle/done, resolve that runtime wait item. A terminal exit ends that wait but may create a separate recovery issue if its assignment lacks an outcome. Disconnection alone does not resolve it.
5. Keep the current view open. Decrement the global count without automatically returning the user to the list or switching to a new item.

A runtime response authorizes continuing the current invocation; it does not approve a task-level gate. Do not add a generic Continue-workflow click solely because a terminal permission was answered. If Canban also holds a task gate, that separate item remains pending.

### Task needs decision

Open Card to read the blocked report, task context and Timeline. The user can add context through a comment/@Agent, open the terminal, choose rework or explicitly continue. Sending a reply/opening a Card does not itself resolve the gate. A live blocked Agent must be answered or ended before a retry; an Agent completed report can make the pending transition ready but cannot approve it.

Expose **Continue workflow** only with the appropriate readiness/help text; use the same AppServer operation and revision checks as Card details. Keep the reason visible if continuation cannot yet run. A send receipt is not proof the decision was acted on.

### Review requested

Open the existing focused Review workspace (Card context plus report/Diff). The user may ask an Agent, return to Develop, explicitly approve workflow continuation, or accept code with the existing commit/merge checks. The list itself does not offer one-click blind merge. The default Review has no next stage; approval need not archive the Card.

### Return destination

Opening Card/Terminal/Review from Overview stores the originating selection/filters/scroll. Its back action returns to **Overview**. Opening work from Board still returns to Board. Jumping to an item may select its Project for that Card/terminal, but it must not convert the global list into that Project's filter.

If the selected item resolves while its context is open, keep a small **Handled** state until the user leaves or selects another item. Avoid removing the selected panel mid-read. The pending list/count can update independently.

## Item lifecycle and duplicate handling

The pending view is derived from current workflow gates, report/recovery facts and runtime waiting episodes. Historical Notices/Timeline events are supporting history, not an instruction to resurrect every old unread record.

| Signal/action | Pending-item effect |
| --- | --- |
| Same blocked observation repeated | Update/coalesce the same runtime waiting episode; no new rows/count |
| Blocked message/kind changes within the same wait | Refresh context; preserve selection; do not depend only on `status_ms` |
| Runtime returns working/idle/done with valid identity | Resolve runtime wait only |
| Agent reports Canban blocked | Create/update task-decision gate item with the actual request/report |
| Agent later reports completed | Retain task gate; update result/readiness |
| Stage requires human review | Show Review requested, associated with the actual report/profile |
| Explicit human continue/rework/retry/acceptance | Recompute the affected task item from authoritative saved state |
| View/read/notice acknowledgement | Does not resolve a pending task or runtime wait |
| Core/connection lost | Keep issue unavailable/unverified; do not imply resolution |
| Archive/restore | Recompute from current lifecycle; old Notices alone must not create phantom pending work |

Use stable source references: Card gate/report for task items; Core ID + terminal address + request/attempt + waiting episode for execution items. A later blocked episode is a new wait, even when it reuses the same terminal. A prior invocation must never resolve a newer invocation's item.

The same Card may have an independent task decision and one or more Agent input waits. Keep these reason records independently resolved. Overview attaches reasons to the actual initiating Card/profile row; a row with several reasons counts once, while distinct profiles remain distinct rows. Deduplicate repeated observations of one source episode, not unrelated reasons merely sharing a Card.

Sorting can start with most recently created waiting item first. Repeated polling/message refresh does not move an item to the top every time. Selecting an item does not change its unresolved status.

## Minimal architecture and reusable components

```text
Canban reports / gates / recovery ----+
                                     +-> AppServer attention projection
Herdr observed root status ----------+          |
                                                +-> global unresolved summary
                                                +-> typed item/context query
                                                +-> existing Card/work navigation
                                                +-> UI refresh event
```

Keep the behavior within existing AppServer/Notifications/Herdr/UI boundaries. A new generic workflow engine, message broker or Session class is unnecessary. Persist the small runtime waiting-episode/history data needed for stable identity and restart reconciliation; rebuild current validity against the actual terminal before declaring it resolved. Transport events wake readers; required workflow gates remain durable product state.

UI components should include the shared Overview navigation indicator/grouped row, selected context panel and bounded terminal-context preview. Reuse navigation/status/notice primitives, actual Card/report components and the existing focused Terminal/Review workspace. Do not duplicate permission controls or make another terminal implementation. Gallery fixtures must cover long context, multiple Projects, missing fields, unavailable Core, simultaneous task/runtime items and a selected item resolving.

## First implementation slice and acceptance

1. Global Overview entry/count and grouped cross-Project Agent list, including existing task gates/recovery/review states.
2. Runtime waiting episodes from Herdr, with source labels and selected-item screen preview.
3. Exact Card/Terminal/Review jump and return-origin preservation; per-source resolution and deduplication.
4. Native Gallery/application acceptance; macOS notifications remain excluded.

Acceptance scenarios must demonstrate a permission/auth/question wait from one Project while viewing another; repeated root observations producing one item; runtime resumption leaving a separate Canban gate untouched; task reply/completion not silently approving a gate; missing runtime metadata falling back to bounded context; obsolete invocation/Core identity never receiving input; loss/reconnect not appearing handled; and list/selection state surviving cross-Project navigation.

Status-based slot release and idle Terminal reclamation share these runtime facts but remain separate policies. Resolving an attention item does not itself kill a terminal, start a duplicate Agent, mark a task completed or merge code.

## Implemented closed-runtime handling

A confirmed unreported exit becomes Failed history, with saved output and an explicit Resume action when available. It clears the runtime input wait and does not create an automatic task decision. Explicit task blocking and human Review remain pending. Disconnect is unconfirmed; a locally authoritative missing record preserves its reason without inventing an exit code. See [event-driven observation](../runtime-observation.md).
