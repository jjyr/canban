# Card-scoped Agent conversations and native terminals

Status: implemented; current contract reconciled 2026-11-10. One provider conversation per (Card, profile), interactive launch/exact Resume, immediate live mentions with explicit CLI replies, and native libghostty-vt Terminals. Completed Agents remain interactive until idle reclamation or explicit closure. Dated real Codex and deterministic acceptance are linked from [implementation](../implementation.md); Claude's preset exists, but live Claude acceptance still requires valid authentication. This documentation update adds no new runtime evidence.

## Product model

A profile is the configured Agent identity/role, such as developer or reviewer. It is distinct from the provider binary/model, Request and terminal process. Different Cards have separate conversations; different profiles on one Card also retain separate provider histories. Two stages referencing the same profile deliberately share context on that Card.

```text
Card #42
  +-- developer (Codex) -> conversation S1
  |      + Develop request -> PTY t1 -> completed, retained while idle
  |      + @developer Q ---> live input to t1; durable reply for Q
  |      + after exit -----> exact Resume of S1 in a new PTY
  |
  +-- reviewer (Codex) --> conversation S2, independent context
         + Review request -> PTY t2

One execution model; trigger and callback differ, not the Session class.
```

[Sessions](sessions.md) defines durable Conversation/Request/Attempt identity and the independent outcome/runtime/connection/human-gate dimensions. Provider conversation IDs are opaque exact bindings; Canban never substitutes a latest-session search or Herdr terminal address.

## Profiles, templates and placement

Profiles live in the application-wide `~/.canban/config/agents.json` registry. They own launch/resume commands, role instructions, aliases and normal-exit configuration. Embedded presets provide editable defaults; users can replace complete command arrays or supply custom presets/profiles. Runtime invocation uses the same command interface for Codex, Claude and custom programs; Herdr owns PTY input, attachment, status and interruption. Canban installs no provider Stop/notify hooks. See the [profile schema](agent-profiles.md).

Workflow templates own stages, task instructions, completion transitions and profile references. Explicit @profile selects that identity; automatic routing uses stage.profile, then global default_agent. Cards have no routing-tag field or alias mapping. Mentions do not replace a stage callback. Profile reload affects subsequently captured execution configuration without hot-changing running or queued snapshots or resetting conversation identity.

New execution uses the Project's current work location at admission. Retained questions/Resume return to the original execution machine, directory and provider history. Review/delivery retain the saved Card workspace. Changing Project location does not move existing work. See [multi-machine execution](../multi-machine.md).

## Entry points and navigation

| Action | No conversation yet | Live conversation | Exited conversation |
| --- | --- | --- | --- |
| Explicit Choose Agent / Start / Resume | Queue an interactive start | Attach/focus existing invocation | Queue exact Resume |
| Submit @profile question | Start with question | Save and immediately deliver to that invocation | Resume with question |
| Enter an executable workflow stage | Queue stage assignment | Reuse a compatible completed idle invocation, otherwise wait for a safe boundary | Resume with fresh assignment/callback |
| Browse Card/history/Overview or Back/Forward | Read context only | Read context or navigate to live viewer | Read saved history; no implicit Resume |

An existing live Card Agent link opens its Terminal. An exited Agent action is explicitly labelled Resume. **Open Card Terminals** selects existing Card context, or an empty Card-context Terminal view if none exists; **Choose Agent** then explicitly invokes `agent.open`. Historical Timeline output opens that execution's saved report/output, not a stale live address. All starts/resumes share Scheduler admission, default global budget eight and human-gate rules; manual Shells do not consume Agent slots.

## Workspace and controls

```text
Board / Card conversation     Overview (all Projects)
  task + context + Timeline     assignment + actionable reasons
  comments / @profile           Card / Review / Terminal actions
             \                  /
              +--> Terminals <--+
                   Project-grouped list | tabs + Card/branch chrome
                   managed Agents      | one native PTY viewer
                   independent Shells  | direct input / permissions
```

Card keeps a compact header and one scrolling task/Timeline region with a reachable composer. Agent lifecycle management belongs in the dedicated Terminal workspace; Review uses its report/Diff/delivery interface. Terminal list rows show Card/custom title, stage, Provider/Profile, textual state and machine inline. Current directory is not displayed in those rows. Selected titles expand; top tabs and branch chrome keep their compact/full-value affordances.

Interrupt, native input/paste and in-turn steering target the actual PTY. Interrupting a still-live Agent does not require Resume. Herdr-reported permission/question waits are handled in that Terminal; Canban does not synthesize Allow/Deny controls or answer arbitrary provider dialogs. Confirmed authentication/launch exits become failures, not perpetual requests for input from a nonexistent Terminal.

X, Command+W and Close terminal invoke one AppServer close lifecycle; successful termination removes the tab, while failure/uncertainty leaves it inspectable. Leaving the page, hiding the list or quitting the UI detaches only the viewer. Closing a Card's detail pane is also navigation, not Terminal termination. Background scheduling and status refresh never steal the selected tab or focus.

## Completion, inactivity and continued work

```text
Agent report completed -> save outcome -> callback once / required human gate
                                |
                         Terminal remains live
                                |
                   verified idle/done, no pending work
                                |
                            Inactive
                       /                \
                  human input        idle timeout
                       |                |
                     Active       normal profile exit
                       |                |
                 later idle       remove row/tab
                                        |
                                history + exact Resume
```

Completion is an explicit Canban report, not terminal output, a reply, Herdr done or exit. A completed live Agent can answer follow-ups without rerunning its original callback. A new compatible stage assignment gets fresh Request/Attempt/reporting bindings even when it reuses the PTY. Stale report commands cannot complete the new assignment.

`scheduler.idle_minutes` defaults to 15; 0 disables reclamation. Verified idle/done, an exact resumable history and a configured normal exit are required. Actual user interaction renews activity; simply leaving the viewer open does not protect it indefinitely. Unfinished automatic assignments, outstanding questions, blocked dialogs and unknown/disconnected runtime remain protected. Completed work may be reclaimed while its Card awaits human Review. Normal exit failures remain inspectable, without forced kill or blind resend.

**Show inactive / Hide inactive** controls live idle rows. Exited sessions disappear from tree and tabs regardless of that toggle; saved output and explicit Resume remain in Card/Overview. See [Terminal interaction](../terminal-interaction.md) for exact guards and verification.

## Immediate mentions and durable replies

```text
User @profile Q -> save comment/Question -> resolve Card/profile
                                             + live: deliver now
                                             + absent: start with Q
                                             + exited: exact Resume with Q
                                             + closing: retain Q across close boundary

Agent canban reply -> durable answer linked to Q
Agent canban report -> original assignment outcome and callback
Herdr output       -> terminal observation, not a durable semantic reply
```

Immediate delivery does not interrupt or replace the current task. It still follows Herdr serialization and the provider's actual input handling; Canban cannot guarantee the CLI consumes text immediately. Unknown delivery is visible and not automatically resent. An answer to Q does not complete the stage or approve a human gate. Direct desktop recipients are explicit; ordinary inline prose mentions do not dispatch. See [Card composition](../card-conversation.md).

## History, Archive and acceptance

Captured inputs, reports, replies and output remain immutable execution history. Worktrees stay Card-owned through Archive; clean managed worktrees may be removed after preserving refs, dirty or independently used worktrees are retained. Restore precedes invoking an archived Card Agent and reconstructs its exact retained managed checkout when possible. The archive contains Canban history, not a portable backup of provider-owned history. Missing exact provider history fails visibly; **Start with retained Card context** is an explicit fresh-context action.

Current acceptance references: [real Agent tasks and post-Archive Resume](../real-agent-acceptance-20261109.md), [retained interaction/native checks](../terminal-interaction.md), [runtime exit/reconnect](../runtime-observation.md), and [multi-machine acceptance](../multi-machine-acceptance.md). They record separate provider, transport and UI limits; Chinese IME remains excluded. Earlier immediate-completion-exit, indefinite-viewer-protection, unlimited-concurrency and view-only-tab-close proposals are superseded by the contracts above. Original alternatives remain in [product history](product-log.md) and [interactive lifecycle research](../research/interactive-agent-lifecycle.md).
