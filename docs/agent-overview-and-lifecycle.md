# Agent Overview and lifecycle implementation

Current behavior update, 2026-11-09: [Terminal interaction and navigation](terminal-interaction.md) supersedes immediate completion exit, indefinite viewer protection and ended-session pages. Completed Agents stay interactive until their configurable idle timeout; actual user input renews activity, exited sessions leave the Terminal list/tabs, and exact Resume lives on the Card.

Implemented 2026-11-07. This supersedes the former policy that every retained terminal occupied an execution slot. Board remains the startup workspace. Overview is the global handling destination across all Projects in the current Canban home.

Navigation update 2026-11-08: Overview now directly renders all nonempty groups without filter buttons/state; the Needs attention section is labeled **Needs you**. [Current UI contract and acceptance](workspace-entry-and-settings.md) also covers grouped Settings.

## Automatic admission and retained terminals

```text
                    One persistent Request queue
                stage entry / @profile / open / resume
                                |
                    workflow eligibility + human gate
                                |
                        global execution budget
                                |
                    later stage first, then FIFO
                                |
                  Profile command -> Herdr terminal
                                |
             +------------------+------------------+
             |                  |                  |
       working/unknown     blocked/idle/done    terminal exited
        reserve slot       release slot        release slot
             |                  |
             |            retain Terminal
             |            show current reason
             +------------------+
                status changes are re-observed
```

`scheduler.max_agents` defaults to **8**. It is an automatic launch budget, not a hard ceiling on open terminals. A user can resume existing waiting Agents and temporarily exceed the limit; no work is killed or preempted. New launches wait until usage falls below the limit.

| Verified runtime condition | Slot policy |
| --- | --- |
| Starting, closing or uncertain launch | Reserve |
| Working, missing/unrecognized status | Reserve |
| Disconnected, replaced Core, stale observation (over 30 seconds) | Reserve conservatively |
| Root blocked, idle, done or error | Release |
| Terminal finished | Release |

Canban completion and runtime status are separate. A `done` status cannot produce a business report, approve a gate or mark a Card ready for Review. A retained live/uncertain invocation still prevents a second invocation of the same Card/profile conversation, even after its execution slot is released. Unrelated Herdr terminals are outside this budget.

## Idle reclamation

Configuration and Settings expose `scheduler.idle_minutes`, default **15**, with **0** disabling reclamation. Existing JSON files get the default without migration. Scheduler configuration preserves other settings and profile commands.

A candidate needs verified live root idle/done, a bound provider session ID, a resume command and a normal-exit input action. The timeout uses the later of the observed idle boundary and actual human interaction. A renewed working state stops idle candidacy; reconnect/restart requires fresh observations and a new idle interval. Opening/viewing alone does not indefinitely protect the process.

Reclamation excludes unfinished automatic stage assignments, outstanding @questions, blocked/auth/question/permission dialogs and working/unknown/disconnected Agents. An unresolved task gate protects unfinished work; a completed Agent may be reclaimed while the Card awaits human Review. Human input through the framed relay renews activity; automatic VT replies, resize/output acknowledgements and viewer presence do not provide indefinite protection. No separate Session class is introduced.

The normal-exit intent is saved before sending input. Herdr's `if: idle` condition is used where supported; its public conditional schema has no `done` variant, so a done Agent is rechecked immediately before its ordinary, unforced exit command. Herdr refuses blocked dialog input without force. There is no atomic cross-process viewer/terminal transaction; external activity can race the last status check. Completion itself no longer sends exit; the provider-defined exit action is used after the inactivity deadline. Explicit tab closure instead uses the separate AppServer close-and-terminate operation.

Exit is confirmed by observing the original Core/terminal finish. No forced kill or blind retry follows a failed/uncertain normal exit. The failure appears as recovery context. An idle exit produces `idle_closed` invocation history, not a completed report or workflow transition. Provider history/ID is retained. A pending question racing closure is retained and queued against the same conversation for exact resume.

## Global Overview

Overview sits below Board in the compact rail. Project controls live in Board; Settings is in the bottom utility bar. The inspector uses the same destinations and lifecycle operations as Card and Terminal views.

```text
Compact rail               Global grouped list           Context inspector
+-------------------+   +--------------------------+   +-------------------------+
| Board             |   | Needs you                |   | Project / Card / profile|
| Overview ! N      |   | Ready for review         |   | Independent reasons     |
| Terminals         |   | Working                  |   | Open Card / Terminal    |
| Notes             |   | Queued                   |   | Review / exact Resume   |
| Archived Cards    |   | Idle                     |   | Assignment / @questions |
+-------------------+   | > Closed                 |   | Saved result / history  |
                        +--------------------------+   | Herdr title/message     |
                                                       | Read-only screen preview|
                                                       +-------------------------+
```

`canban-notifications::overview` is a pure typed projection shared by AppServer and UI. It has no Core/UI calls. One row represents a Card/profile conversation, with retained live/uncertain work taking precedence over queued followups. Old invocations appear in expandable history. Routing/preparation failures without a matching invocation have a Card recovery row.

The global badge counts actionable **rows**, including Review. Several independent reasons on one row count once. A Card gate attaches to its initiating profile rather than every Agent on the Card. Runtime permission waits are persisted/deduplicated by episode, including kind/title/message changes while the state word stays blocked. These produce Notices and Timeline events without creating a business gate. Resolving a runtime wait never approves an existing task gate. Reading a notice or visiting Overview is not resolution.

Selection only opens context. Explicit actions attach the exact live invocation, open the actual Card/Review, or request exact-ID resume through the Scheduler. Resume validates the selected attempt and its bound provider ID before queueing. Missing provider history/ID, changed invocation or removed resume commands require explicit recovery rather than a silent fresh session.

Selected conversation, Closed expansion, split width and list scroll survive Card/Terminal/Review navigation. Destinations opened from Overview return to Overview; Board-origin work retains Board navigation. On narrow widths the inspector replaces the list and has a close/back action. A selected context remains visible if its condition resolves or its group changes. Closed is collapsed initially.

The screen preview is loaded on demand, read-only, at most 160 lines and 32 KiB, and bound to the selected Core/terminal/attempt. Server identity checks plus UI generation fences reject obsolete results. Terminal text is selectable plain text, never a synthesized permission form. Permissions are answered in the actual Terminal; there are no inferred Allow/Deny actions. macOS notifications, host statistics and snoozing remain deferred; machine connection management follows the [multi-machine contract](multi-machine.md).

## Public operations

```sh
canban call scheduler.status '{}'
canban call scheduler.configure '{"max_agents":8,"idle_minutes":15}'
canban call overview.list '{}'
canban call overview.preview '{"row_id":"CARD_ID:PROFILE_ID","attempt_id":"REQUEST_ID:1"}'
canban call agent.open '{"card_id":"CARD_ID","profile_id":"developer","expected_attempt_id":"REQUEST_ID:1"}'
```

`overview.list` returns typed rows with group, Project/Card, request/attempt, independent reasons and history IDs. Preview returns text, capture time and exact source identity. Existing generic `canban call` exposes these operations; no new Agent provider protocol or MCP service is required.

## Earlier validation (2026-11-07/08)

These results describe the original implementation, including viewer-based idle protection and the former Theme modes. That protection is superseded by [retained interaction acceptance](terminal-interaction.md#verification), and Theme modes by [named-theme acceptance](desktop-utilities.md#named-theme-acceptance-2026-11-09). Keep the recorded results as historical evidence, not the current contract.

- Rust regression coverage: conservative slots, retained-conversation exclusivity, above-limit resumption, full runtime wait deduplication, independent task gates, timeout/activity/viewer/resume guards, global grouping and gate ownership, configuration preservation.
- `mise run acceptance-overview`: actual isolated Herdr PTYs with deterministic provider fixtures, two Projects, permission waits/updated context, manual resumption above capacity, idle/done slot release, independent task/runtime resolution, bounded preview/stale identity rejection, one-minute normal idle exit and exact-session resume. Final passing evidence (including both idle and done discussion reclamation, with unfinished stage protection): `/private/tmp/cb-overview-fkxtkjsi/evidence.json`.
- `mise run acceptance-scheduling`: default eight, remote/base/worktree synchronization, later-stage priority, persistent human gates and limit edits. Updated passing evidence: `/private/tmp/cb-scheduling-2gcngzbj/evidence.json`.
- Native acceptance uses a packaged app and isolated Core at `/private/tmp/cb-native-muzjif13`: cross-Project rows, persistent badge, permission context/preview, Terminal input and return, saved Review grouping, exact-ID Resume retaining the human gate. Runtime snapshot: `overview-ui-evidence.json` in that directory. Gallery covers all groups, recovery without a request, long context, themes and constrained layout.

These are deterministic providers running through real Herdr terminals, not new paid/live Claude/Codex task acceptance. Chinese IME remains explicitly excluded; Claude authentication remains a separate existing prerequisite.

Final checks: 58 Rust tests passed, all-target Clippy with warnings denied, main/Gallery builds, formatting, diff whitespace and Python syntax checks. General acceptance passed at `/private/tmp/cb-6uo6vrpk/home`; interactive/recovery acceptance passed at `/private/tmp/cb-interactive-0f0z7z1l/home`. Native viewer protection exceeded the configured one-minute timeout (99 seconds observed); leaving the Terminal restarted the timeout and then confirmed normal idle exit. Evidence: `/private/tmp/cb-native-muzjif13/viewed-idle.json`. The final Gallery checked light/dark appearance, wide/narrow inspector navigation and invocation-history disclosure.

## Exited Agent recovery (2026-11-09)

Current policy: the [event-driven runtime update](runtime-observation.md) records Failed history for an unreported exit and removes only the historical automatically generated missing-outcome gate; explicit task/Review decisions remain pending. The UI/output behavior below still applies when an independent task gate exists or the user opens Closed history. Overview now labels the invocation **Agent exited · exit code N** (when known), distinguishes it from live **Agent needs Terminal input**, and explains that the remaining action belongs to the task. It offers Open Card / exact Resume as appropriate rather than Open terminal for a finished invocation.

Finished invocations use **Read saved output**. `overview.preview` reads the Card-owned captured output directly without querying the original Core, bounds the returned UTF-8 text to 32 KiB and preserves the exact invocation check. Its source is `Saved execution output (read-only)` and its `observed_at` is null; a saved file is not presented as a freshly observed runtime screen. Missing saved output is shown honestly. `execution.output` also prefers saved output for finished invocations. Live read-only previews retain their existing Core identity/freshness guards.

Card's Needs attention shortcut now selects its actual actionable Overview row; a conversation-level gate does not necessarily have a `card:<id>` row. A row removed from the current projection displays a handled message instead of exposing actions from a cached historical selection.

Historical validation before the final event-driven exit policy:

These checks exercised the earlier automatic missing-outcome gate. The [event-driven verification](runtime-observation.md#verification) supersedes that classification, proving runtime-wait removal and Failed history without a new task gate. Saved-output/Resume checks below retain their original scope.

- 131 Rust tests passed, including a confirmed exit with an old blocked status, the preserved task gate, bounded Unicode saved output with an unavailable Core, stale-attempt rejection and Closed grouping after the task gate is handled.
- `mise run acceptance-overview` passed with isolated real Herdr and deterministic providers: permission wait -> unreported exit -> task-only attention, captured output, Core shutdown -> readable saved context, one-minute idle exit and exact Resume. Its launch wait now requires the provider binding as well as the execution receipt.
- The production-component Gallery includes an exited-before-reporting fixture alongside live input waits and disconnected recovery.
- Native isolated App: Card attention opened the correct context, exit code 1 and Read saved output were visible, saved failure output loaded after the original Core stopped, exact Resume preserved the provider session and accepted `FOLLOWUP_AFTER_RESUME`. A subsequent exit removed the Terminal row/tab; after UI restart the saved output had the correct source label and retained the follow-up text.
- No real Claude/Codex invocation was started, and no human gate in the user's regular home was changed.
