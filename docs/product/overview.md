# Global Agent Overview

Status: implemented; current contract reconciled 2026-11-10. See [implementation and validation](../agent-overview-and-lifecycle.md) for current behavior and evidence. The user-selected design below remains the product contract; macOS notifications and host resource monitoring remain deferred; machine connections/execution are covered by the [multi-machine contract](../multi-machine.md).

## Reference and selected direction

The screenshot is the Herdr UI **Overview**. The inspected checkout is `~/Workspace/herdr-gpui`, revision `a6db99b4`, with a clean tracked working tree during inspection. Its main references are:

- `crates/herdr-gpui/src/overview.rs`: gathers agents across endpoints and failed orchestrator runs from prepared state.
- `crates/herdr-gpui/src/overview/model.rs`: typed rows, stable keys and state grouping.
- `crates/herdr-gpui/src/overview/view.rs`: summary, highlighted attention group, Ready for review, Working, Idle and collapsed history; branch/host/time columns and row actions.
- `crates/herdr-gpui/src/overview/hosts.rs`: collapsible host/load column.
- `crates/herdr-gpui/src/overview/closed.rs` and `reopen.rs`: closed-agent history and reopening behavior.
- `crates/herdr-gpui/src/sidebar/activity/counts.rs`: the activity entry's attention count, independent of total live agents.

Adopt the global grouped list, compact scanning hierarchy, attention prominence, status duration and closed-history affordance. Keep Canban's own Card/profile conversation identity, task history and exact provider-ID resume. The reference reopening code can start an Agent command in a new tab; that is not a substitute for Canban's exact resume contract. GPUI view code is not an NSH component implementation to copy directly.

## Product role and navigation

Board remains the default workspace for defining and advancing work. **Overview** is the global execution/handling workspace across Projects. It shows working, waiting, queued, idle and closed Agent conversations plus actionable Cards that have no Agent yet.

The **Overview** entry sits immediately below Board in the compact rail, replacing the earlier Agent activity/separate Needs attention destinations. It directly shows all nonempty status groups without filters. Board also has no attention/execution filter toolbar; it shows all active Cards for its selected Project.

```text
Quick Open
Board
Overview !4  <- global user-action count
Terminals
Notes
Archive

Bottom utility bar: Usage / Caffeine / Theme / Shortcuts / Report issue / Settings
```

The entry is always visible. Its badge now counts unique active Cards with unread Agent updates or unresolved actions, using [unified reminders](../reminders.md). Reading clears only unread information; authoritative action state persists until resolved. Visiting Overview is neither reading a reply nor approving a gate. The runtime groups and execution summary retain their action/Agent semantics.

Changing Project does not scope Overview. Background updates do not steal focus. No macOS notification integration is required.

Project controls belong in Board, and Settings belongs in the bottom utility bar. See [Project entry](../workspace-entry-and-settings.md) and [desktop utilities](../desktop-utilities.md).

## Layout

```text
+------------+-----------------------------------------------+--------------------------+
| Quick Open | Overview - All Projects                       | Selected context [x]     |
| Board      | Needs you / Executing / Queued summary        | Project / #42 / profile  |
| Overview   |                                               | Permission required      |
| !4         | NEEDS YOU                                     |                          |
| Terminals  | @developer #42 Add search / canban / Develop  | Captured assignment      |
| Notes      | @developer #17 RPC / fiber / Develop          | Original @question/task  |
| Archive    |                                               |                          |
|            | READY FOR REVIEW                              | Herdr title/message      |
|            | @reviewer #8 Export data / tools / Review     | Bounded terminal preview |
|            |                                               | or saved output          |
|            | WORKING / QUEUED / IDLE                       |                          |
|            | (nonempty groups; no filter toolbar)          | [Terminal] [Card/Review] |
|            | > Closed                                      |                          |
+------------+-----------------------------------------------+--------------------------+
|            | Bottom utilities / Settings                                              |
+------------+--------------------------------------------------------------------------+
```

The grouped list is the default primary surface and fills the available space when no context is selected. A context inspector opens on selection and can collapse. This adapts the reference's right-hand column to Canban's immediate need: Card/assignment/permission context. CPU/memory/ports/remote-host monitoring can follow later and should not be prerequisites for this page.

Rows use two lines: task/question title first; Project, Card, profile, stage and concise reason second. Show branch and state duration in aligned secondary columns while space permits; move them into the inspector at narrow widths. Use status text and a glyph as well as color. Keep empty groups hidden, Closed collapsed and the user's scroll stable.

A single click selects context. An explicit Terminal/Card/Review action jumps directly to the target. Avoid starting/resuming a process merely because a row was selected for reading. Selected-row actions remain visible without relying only on hover. On narrow widths, context replaces the list with a clear return action.

## Row identity and grouping

The canonical Agent row is a **Card/profile conversation**, projected from its latest relevant request/attempt. Different profiles working on the same Card stay distinct. Older attempts are expandable history, not another primary row for every event/run. A pending request without a terminal still has a row. If routing/preparation failed before an Agent/request exists, include a Card-level recovery row so the error is not hidden by an Agent-only list.

An attention condition attaches to its actual initiating invocation/profile; do not repeat a Card gate on every Agent associated with the Card. Independent reasons remain independently resolved. If one invocation has both a task gate and a permission wait, show both reasons in one row; count that actionable row once. Distinct Agents with their own waits remain distinct rows. The row/inspector may expose several reasons without losing their individual source identities.

Choose one primary group per row, in this order:

| Group | Source/meaning | Primary destination |
| --- | --- | --- |
| Needs you (`Needs attention` API key) | Task needs decision, runtime input wait, actionable error or unverifiable recovery | Card or exact Terminal according to reason |
| Ready for review | Saved Card/stage result has an actual human review gate | Existing Review workspace, or Card for a non-code review |
| Working | A current invocation is starting/working or closing, without a higher-priority reason | Attach exact live Terminal |
| Queued | No current live invocation; a request awaits scheduling | Read queued assignment/Card; do not bypass capacity |
| Idle | A verified live Agent is idle/done, with no outstanding action | Attach existing Terminal |
| Closed | No live/pending invocation or unresolved action; saved conversation remains | Explicit exact-ID Resume |

Rows in higher groups can still show secondary execution facts (for example, task decision pending while a follow-up Agent works). The execution-slot summary comes from the Scheduler's own policy and is not the number of rows in the Working section. Unknown/unreachable runtime is not Idle. Preserve an unavailable/recovery presentation rather than copying the reference model's Unknown-to-Idle simplification.

A Herdr `done` report alone must **not** become Ready for review. Herdr observes runtime; Canban requires its own saved outcome and workflow/human-review state. A Reviewer may have exited normally while its row remains Ready for review. Likewise, a closed terminal with unresolved task blockage stays in Needs attention. Resolving the condition then allows the row to move to its current lifecycle group.

## Context and handling

The inspector includes:

- Project, Card number/title and stage.
- Profile plus provider display name, current execution/connection state and waiting/state duration.
- The actual stage assignment or original @question for that invocation, including report/comment links. Keep captured assignment distinct from current mutable Card text.
- Source-specific reason: Execution waiting (Permission/Sign-in/Answer required), Task needs decision, Review requested or Recovery needed.
- Herdr-provided title/message when present; an on-demand bounded read-only terminal screen when those fields are insufficient.
- Branch/worktree and relevant delivery/result details, with technical IDs secondary.

Herdr does not promise a structured approval payload. The inspector labels reported text versus terminal preview and offers **Open terminal**, not generic unverified Allow/Deny controls over text. Task decisions use Card comments/@Agent and explicit workflow continuation/rework. Human Review uses the existing report/Diff/acceptance surface.

Detailed per-source lifecycle, deduplication and runtime-context rules remain in [attention handling](needs-attention.md).

## Navigation, recovery and history

Opening Card/Terminal/Review from Overview preserves the originating row and scroll; Back returns to **Overview**. Opening from Board continues to return to Board. Selecting the target Project while opening its Card must not scope the global Overview afterward.

Runtime status can resolve an execution-wait reason, but cannot approve a Canban task gate. Agent completion alone does not approve a task gate. Reading/selecting a row does not settle it. If its reason resolves while context/Terminal is open, update the summary without closing or switching the user's work surface. Preserve a small handled state in the selected inspector until the user moves on.

Closed means the invocation ended, not that its provider history was deleted or Card archived. Resume uses the existing Card/profile binding and command-defined profile, under Scheduler admission. Missing history and archived Cards keep the current explicit recovery/Restore rules. Disconnection is not evidence that a terminal closed. The reference's client-local closed-row retention/keying must not replace Canban's durable Card/profile/request/attempt associations.

Idle reclamation moves verified unused Agents to Closed while retaining history. Its implemented timeout/activity guards live in [Terminal interaction](../terminal-interaction.md). Finished sessions leave the Terminal workspace tree/tabs; Closed history remains available here and on the Card. A confirmed unreported exit becomes Failed history, not a new human gate or a stale live input request. Independent task/Review gates can still keep that row actionable.

## First implementation boundary

Reuse existing NSH UI Kit navigation/status/splits and production Card/Terminal/Review components. Add a typed Overview projection, summary, grouped row and context inspector. Build rows from prepared AppServer state; no render-time remote/Git/process calls. Load detailed terminal context asynchronously only for the selected row, with identity/generation fences.

The first version should cover all Canban-managed conversations across Projects, plus their queued/recovery/Card review items. Do not list unrelated terminals from the user's Herdr account without an explicit product decision. Local/remote machine identity can be shown as metadata. Machine setup and routing follow the existing multi-machine implementation; CPU/memory monitoring remains deferred.

Pinned/Snoozed/Settled shelves, notification mutes, presets and forgetting provider history are not needed for the first slice. Hiding a row must never be an implicit workflow approval. Keep exact resume, task/runtime separation and Card association from the current design.

## Acceptance plan

Use Gallery fixtures and an isolated real-Herdr application to check:

1. Multiple Projects and profiles appear in one list; current Project selection never hides others.
2. The global reminder badge counts Cards once across unread and pending items, independently of running/idle/closed Agent row counts.
3. Runtime blocked, Card blocked and Review gates group correctly; Herdr done/unknown does not fabricate review readiness/idle.
4. Queued requests do not start from row selection, and Terminal attach does not create a duplicate Agent.
5. Closed conversations resume their exact provider history; unavailable/missing history remains explicit.
6. Context identifies the actual Card and question/stage task; missing Herdr fields use a labeled bounded preview.
7. Repeated status updates do not duplicate rows, reorder them continuously or reset scroll/selection.
8. Card/Terminal/Review back navigation returns to Overview; narrow layout retains the same state.
9. Resolving a permission wait leaves an independent task gate pending; closed Reviewer output remains available for human review.

Source inspection alone is not runtime acceptance. The implementation now has isolated Herdr integration and native UI checks recorded in [Agent Overview and lifecycle](../agent-overview-and-lifecycle.md).
