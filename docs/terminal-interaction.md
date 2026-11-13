# Terminal interaction, inactivity and window navigation

Status: implemented, 2026-11-09. This is the current contract and supersedes immediate exit after an outcome, the ended-session replacement page and the bottom Show/Hide ended control.

## Visible workspace

```text
+-----------------------------+----------------------------------------------+
| traffic lights  [<] [>] list | [Card/profile x] [Terminal 1 x] [+] Card/branch |
+------+----------------------+----------------------------------------------+
| rail | Terminals            |                                              |
|      |                      |             Native Terminal                  |
|      | v [folder] Project   |                                              |
|      |   | Card title   [!] |                                              |
|      |   | provider/profile |                                              |
|      |   | Terminal 1       |                                              |
+------+----------------------+----------------------------------------------+
```

Project groups use a shared folder vector, muted semibold typography, spacing and indented child guides. Selection belongs to the Terminal row. Local Shell rows omit the AppServer host caption; remote names remain useful compact context. Full directory/Core/machine values stay in tooltips/actions. Manual names remain Terminal 1, Terminal 2, etc.; Agent names retain Card/stage/provider/profile.

All live terminals stay visible in the sidebar until closed or reclaimed; there is no Show/Hide inactive toggle button and no inline Active/Inactive text next to terminal rows or tabs. `Active` vs `Inactive` is strictly an internal lifecycle state for managed Card/Workflow Agents (manual Shells and independent CLI sessions do not participate in automatic idle reclamation). Whenever a terminal needs user inspection (permission/input `blocked`, unfinished/interactive Agent turn sitting `idle`/`done`, runtime `error`, or disconnected), an attention icon (`!`) is rendered on its sidebar row and tab alongside notification chimes. Finished processes and archived Card sessions leave both tree and tabs; saved history and Resume remain on the Card.

## Completion, interaction and delayed exit

```text
explicit outcome -> Workflow result/progression
                          |
                   process stays available
                          |
                idle/done + no pending work
                          |
                      Inactive
                     /        \
              human input    idle timeout
                  |              |
                Active      profile exit or PTY close
                  |              |
           idle/done again   remove row and tab
                                 |
                      history retained on Card
                                 |
                         explicit Resume / reopen
```

The scheduler idle timeout remains configurable in Settings and agents.json (default 15 minutes; 0 disables it). Completion no longer sends an immediate exit command. Herdr runtime status is still required; a report alone cannot prove the provider is idle. Unfinished automatic assignments, permissions/blocked states, unknown/disconnected runtimes and unresolved Questions stay protected and immediately alert the user (`Needs you`). Completed assignments (and long-idle interactive Card sessions) are reclaimed after `idle_minutes` across all Agent profiles: profiles with an input `ExitAction` request normal exit first (falling back to clean PTY termination if still idle after 30s), while profiles with `ExitAction::Manual` terminate the verified-idle PTY directly after capturing output. Manual Terminals (`conversation_id == None`) are never auto-reclaimed.

Opening a viewer does not indefinitely prevent reclamation. AppServer's framed relay observes actual user input/focus and refreshes its activity timestamp; automatic protocol replies, geometry changes and output-consumption acknowledgments do not refresh it. Idle admission uses the later of the runtime idle timestamp and last human interaction. Input while the process remains live postpones exit. A renewed working state stops idle candidacy. Uncertain exit delivery is recorded rather than automatically resent or force-killed.

Archive fences new launches, closes live associated Agent PTYs through their recorded Core bindings, captures final output and then packs Card history. A close failure leaves an explicit archive error for retry. Independent Shells in a worktree retain the existing cleanup protection. Archived rows/tabs disappear; reports and provider conversation identifiers remain in durable history.

## Retained assignments and replies

A completed live Card/profile invocation accepts later @questions directly. These replies do not rerun its completion callback. A later Workflow assignment in the same profile/environment can reuse an idle PTY: it creates a new Request/Attempt with an immutable captured context, marks the preceding Attempt continued, and keeps one physical runtime owner for scheduling. Core/terminal viewer identity stays stable across this transition. Other profiles keep their own conversation.

The new assignment prompt supplies explicit request/attempt/run-directory bindings because a live process's launch environment is immutable. Stale new reports against a continued Attempt are rejected; existing report receipts remain idempotent. Ambiguous assignment delivery becomes an inspectable unknown execution, not an automatic resend. Changed launch commands or a busy/uncertain existing invocation continue to require safe admission rather than creating duplicate conversations.

## Global navigation

Window-local navigation.rs owns Back/Forward history for Project/page/Card/Note and selected Terminal destinations. All window chrome uses the same controls and global Command+[ / Command+] commands. Existing Terminal tab-cycle shortcuts remain Command+Shift+[ / Command+Shift+]. Route mutations within one UI event are coalesced; revisiting after Back truncates only the forward branch.

Traversal restores selection and the existing window-owned drafts/scroll positions, closes transient overlays, invalidates stale async output and restores Terminal focus. It never repeats Agent launch, Card mutation or another command. Closed/archived Terminal destinations resolve to Card context instead of restarting a process. Missing Cards fall back to their Board. History and drafts are in-memory per window, not persistent application history.

## Input and rendering

The native component now sends human input on a dedicated blocking-receive worker, independent of output polling. First-connection keystrokes are buffered for that binding; disconnected input is discarded and never replayed on reconnection. Ordinary pending product operations no longer globally disable an already attached Terminal. Explicit overlays and connection/closing gates still suspend input.

The AppKit viewer checks updates at up to 60 Hz, invalidates changed rows/cursor/status regions, and skips unrelated rows while drawing. Frame JSON serialization occurs after releasing the VT-model lock. The terminal remains a libghostty VT model with an AppKit renderer; this change does not claim a GPU renderer or measured end-to-end display latency.

## Verification

- Workspace tests, strict all-target Clippy and isolated standard/interactive acceptance passed. Updated interactive scenarios cover immediate replies, completed-session replies, explicit close/exact Resume, Core/service recovery and archive history.
- `mise run acceptance-retained` passed against isolated real Herdr 0.1.18 with a deterministic provider. Same-profile Develop/Review used one physical PTY with separate execution records. Later comments replied without repeating Workflow progression. Real human frames renewed an artificially aged idle record; a one-minute timeout still reclaimed it with a viewer open. Exact Resume and Archive closing/output restoration passed.
- The same harness measured 20 local relay-to-fixture round trips: median 0.11 ms and P95 0.24 ms in the recorded run. This excludes native key handling/rendering, provider inference and remote-network latency; it is transport evidence, not a UI latency benchmark. Evidence: /private/tmp/cb-retained-vl6vx027/evidence.json.
- Native disposable Canban/Core: Shell echo, rapid-tab-switch first-input retention, 3,000-line output followed by successful input, Card draft retention across Back/Forward, Terminal-focus Command+[ / Command+], live completion remaining in the Terminal, timed disappearance of rows/tabs, Show inactive excluding exited sessions and Card-based exact Resume were checked. User live data/Core were not used. Chinese IME was excluded at the user's request.
- Gallery includes Inactive Terminals using the production frame/tree. It covers hidden/live inactive rows, the toggle and retained manual/attention entries.

The paid-provider scripts use an explicit one-minute idle timeout for their existing exact-Resume checks; they were not run in this change. These runs used deterministic Agents rather than new paid-provider acceptance. The prior dated real-model reports remain separate evidence.
