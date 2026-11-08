# Agent Overview and lifecycle implementation

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
| Starting or uncertain launch | Reserve |
| Working, missing/unrecognized status | Reserve |
| Disconnected, replaced Core, stale observation (over 30 seconds) | Reserve conservatively |
| Root blocked, idle, done or error | Release |
| Terminal finished | Release |

Canban completion and runtime status are separate. A `done` status cannot produce a business report, approve a gate or mark a Card ready for Review. A retained live/uncertain invocation still prevents a second invocation of the same Card/profile conversation, even after its execution slot is released. Unrelated Herdr terminals are outside this budget.

## Idle reclamation

Configuration and Settings expose `scheduler.idle_minutes`, default **15**, with **0** disabling reclamation. Existing JSON files get the default without migration. Scheduler configuration preserves other settings and profile commands.

A candidate must have a verified live root idle/done status for the entire timeout, a bound provider session ID, a resume command and a normal-exit input action. User input, opening/viewing a native Terminal, a changed idle status or reconnecting resets/protects the idle interval. Restart requires fresh observations and a new idle interval.

Reclamation excludes unfinished automatic stage assignments, unresolved task gates, outstanding @questions, blocked/auth/question/permission dialogs, working/unknown/disconnected Agents and viewed native Terminals. A local transport viewer lease protects the attachment independently of keyboard focus; protocol-generated VT replies do not count as user input. No separate Session class is introduced.

The normal-exit intent is saved before sending input. Herdr's `if: idle` condition is used where supported; its public conditional schema has no `done` variant, so a done Agent is rechecked immediately before its ordinary, unforced exit command. Herdr refuses blocked dialog input without force. There is no atomic cross-process viewer/terminal transaction; external activity can race the last status check. Normal completion uses the same provider-defined exit behavior.

Exit is confirmed by observing the original Core/terminal finish. No forced kill or blind retry follows a failed/uncertain normal exit. The failure appears as recovery context. An idle exit produces `idle_closed` invocation history, not a completed report or workflow transition. Provider history/ID is retained. A pending question racing closure is retained and queued against the same conversation for exact resume.

## Global Overview

```text
Canban sidebar             Global grouped list           Context inspector
+-------------------+   +--------------------------+   +-------------------------+
| Overview ! N      |   | Needs attention          |   | Project / Card / profile|
|                   |   | Ready for review         |   | Independent reasons     |
| Projects          |   | Working                  |   | Open Card / Terminal    |
| Board / Notes     |   | Queued                   |   | Review / exact Resume   |
| Archive / Settings|   | Idle                     |   | Assignment / @questions |
+-------------------+   | > Closed                 |   | Saved result / history  |
                        +--------------------------+   | Herdr title/message     |
                                                       | Read-only screen preview|
                                                       +-------------------------+
```

`canban-notifications::overview` is a pure typed projection shared by AppServer and UI. It has no Core/UI calls. One row represents a Card/profile conversation, with retained live/uncertain work taking precedence over queued followups. Old invocations appear in expandable history. Routing/preparation failures without a matching invocation have a Card recovery row.

The global badge counts actionable **rows**, including Review. Several independent reasons on one row count once. A Card gate attaches to its initiating profile rather than every Agent on the Card. Runtime permission waits are persisted/deduplicated by episode, including kind/title/message changes while the state word stays blocked. These produce Notices and Timeline events without creating a business gate. Resolving a runtime wait never approves an existing task gate. Reading a notice or visiting Overview is not resolution.

Selection only opens context. Explicit actions attach the exact live invocation, open the actual Card/Review, or request exact-ID resume through the Scheduler. Resume validates the selected attempt and its bound provider ID before queueing. Missing provider history/ID, changed invocation or removed resume commands require explicit recovery rather than a silent fresh session.

Selected conversation, Closed expansion, split width and list scroll survive Card/Terminal/Review navigation. Destinations opened from Overview return to Overview; Board-origin work retains Board navigation. On narrow widths the inspector replaces the list and has a close/back action. A selected context remains visible if its condition resolves or its group changes. Closed is collapsed initially.

The screen preview is loaded on demand, read-only, at most 160 lines and 32 KiB, and bound to the selected Core/terminal/attempt. Server identity checks plus UI generation fences reject obsolete results. Terminal text is selectable plain text, never a synthesized permission form. Permissions are answered in the actual Terminal; there are no inferred Allow/Deny actions. macOS notifications, host statistics, remote management and snoozing remain deferred.

## Public operations

```sh
canban call scheduler.status '{}'
canban call scheduler.configure '{"max_agents":8,"idle_minutes":15}'
canban call overview.list '{}'
canban call overview.preview '{"row_id":"CARD_ID:PROFILE_ID","attempt_id":"REQUEST_ID:1"}'
canban call agent.open '{"card_id":"CARD_ID","profile_id":"developer","expected_attempt_id":"REQUEST_ID:1"}'
```

`overview.list` returns typed rows with group, Project/Card, request/attempt, independent reasons and history IDs. Preview returns text, capture time and exact source identity. Existing generic `canban call` exposes these operations; no new Agent provider protocol or MCP service is required.

## Validation

- Rust regression coverage: conservative slots, retained-conversation exclusivity, above-limit resumption, full runtime wait deduplication, independent task gates, timeout/activity/viewer/resume guards, global grouping and gate ownership, configuration preservation.
- `mise run acceptance-overview`: actual isolated Herdr PTYs with deterministic provider fixtures, two Projects, permission waits/updated context, manual resumption above capacity, idle/done slot release, independent task/runtime resolution, bounded preview/stale identity rejection, one-minute normal idle exit and exact-session resume. Final passing evidence (including both idle and done discussion reclamation, with unfinished stage protection): `/private/tmp/cb-overview-fkxtkjsi/evidence.json`.
- `mise run acceptance-scheduling`: default eight, remote/base/worktree synchronization, later-stage priority, persistent human gates and limit edits. Updated passing evidence: `/private/tmp/cb-scheduling-2gcngzbj/evidence.json`.
- Native acceptance uses a packaged app and isolated Core at `/private/tmp/cb-native-muzjif13`: cross-Project rows, persistent badge, permission context/preview, Terminal input and return, saved Review grouping, exact-ID Resume retaining the human gate. Runtime snapshot: `overview-ui-evidence.json` in that directory. Gallery covers all groups, recovery without a request, long context, themes and constrained layout.

These are deterministic providers running through real Herdr terminals, not new paid/live Claude/Codex task acceptance. Chinese IME remains explicitly excluded; Claude authentication remains a separate existing prerequisite.

Final checks: 58 Rust tests passed, all-target Clippy with warnings denied, main/Gallery builds, formatting, diff whitespace and Python syntax checks. General acceptance passed at `/private/tmp/cb-6uo6vrpk/home`; interactive/recovery acceptance passed at `/private/tmp/cb-interactive-0f0z7z1l/home`. Native viewer protection exceeded the configured one-minute timeout (99 seconds observed); leaving the Terminal restarted the timeout and then confirmed normal idle exit. Evidence: `/private/tmp/cb-native-muzjif13/viewed-idle.json`. The final Gallery checked light/dark appearance, wide/narrow inspector navigation and invocation-history disclosure.
