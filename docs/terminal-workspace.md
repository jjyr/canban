# Dedicated Terminal workspace

Current behavior update, 2026-11-09: [Terminal interaction and navigation](terminal-interaction.md) supersedes immediate completion exit, indefinite viewer protection and ended-session pages. Completed Agents stay interactive until their configurable idle timeout; actual user input renews activity, exited sessions leave the Terminal list/tabs, and exact Resume lives on the Card.

Multi-machine update, 2026-11-09: see [current execution/location and service contract](multi-machine.md). Profile commands resolve on their execution host; Resume and Review retain their original workspace, while new work uses the Project's current location.


Implemented user-approved direction, 2026-11-08. Native/runtime acceptance is recorded below. This supersedes the Terminal portion of the earlier Card-focused split layout. Review retains its own delivery interface.

## Product layout (updated 2026-11-15: Compact Two-Line Session Rows, Worktree-Scoped Tabs & Herdr Status)

```text
+--------------------------------------+-------------------------------------------------------------+
| traffic lights [<] [>] [☰]           | [#42 ... x] [Terminal 1 x] [+] [▾] [🤖] #42 [⎇ canban/42 ▾] |
+--------------------------------------+-------------------------------------------------------------+
| rail | Terminals                     |                                                             |
|      | 📁 Canban          [⎇+] [+]   | Native Terminal (active worktree scope only)                |
|      |   📂 main              ↓1 [+] |                                                             |
|      |   │ Terminal 3                |                                                             |
|      |   │ ❯_ Local                  |                                                             |
|      |   │ Terminal 4                |                                                             |
|      |   │ [Codex] Local   ● Working |                                                             |
|      |   ⎇ canban/42 #42 +12 −3 [+][⋯]|                                                            |
|      |   │ #42 Card title truncated… |                                                             |
|      |   │ [Agy] @agy · Local 🛡 Blocked|                                                          |
|      |   │ Antigravity 1             |                                                             |
|      |   │ [Agy] Local        ✓ Done |                                                             |
|      |   │ Terminal 1                |                                                             |
|      |   │ ❯_ Local                  |                                                             |
|      |   ⎇ feat/login    ↑2   [+][⋯] | <-- Active ManagedWorktree (idle ones hidden from tree)     |
|      |   │ Terminal 2                |                                                             |
|      |   │ ❯_ Local                  |                                                             |
|      | 📁 Docs & Website   [⎇+] [+]  | <-- Empty project: single title row only                    |
+--------------------------------------+-------------------------------------------------------------+
```

The window chrome follows the same left/right divider as the content while aligning with the unified 42-point chrome geometry across all screens: native traffic-light clearance and `<` / `>` Back/Forward sit at the exact same horizontal coordinates as every other page, followed by the sidebar toggle (`☰`), so `<` / `>` never jump when switching to or from `Terminals`. The right panel owns **Worktree-scoped tabs** (only live Terminals in the same `WorktreeScope` as the selected Terminal), `+` (New Shell in current worktree), `▾` (Project Root / Managed Branch Worktree / Machine menu), Open Agent, current Card/Project link, the **Project / Worktree Switcher** pill (`[⎇ <branch> · +N −M · ↑N ↓M ▾]`), compact actions, and the global right-end **Activity & Notifications** trigger pill (`[⚡ N · ● active/max]`, `Command+I`). Resizing the left panel moves its full-height divider. With the list hidden, tab controls clear the native window buttons and `<` / `>`. NSH owns window dragging, system double-click behavior, traffic-light geometry and resizable panels. Terminal content has no separate title, toolbar, tab strip, placeholder asking to select a conversation, or permanent technical status footer.

### Unified `WorktreeScope` and context linkage (2026-11-14)

- **Four `WorktreeScope` variants:** `ProjectRoot(project_id)` (main repository directory), `CardWorktree(card_id)` (Card worktree and associated sessions), `ManagedWorktree(worktree_id)` (standalone branch checkout), and `Unassigned(machine)` (external directories grouped under `Other`). Workflow Agents stay pinned to `CardWorktree(card_id)`; Project Workflow configuration sessions stay in `ProjectRoot(project_id)`; Shells and ephemeral CLI sessions resolve dynamically from `TerminalContext.cwd` (`ManagedWorktree -> CardWorktree -> ProjectRoot -> Unassigned`).
- **Scope-isolated Tabs, `Command+1`–`9` & cycling:** The top tab strip, `Command+1` through `Command+9` (selecting the 1st through 9th tab in the current tab strip), and `Command+Shift+[` / `Command+Shift+]` (`cycle`) display and switch strictly across live Terminals within the active `WorktreeScope` (always including the currently selected Terminal itself, with `SELECTED` background, semibold `TEXT` title vs regular `MUTED` sibling titles, and a bottom `ACCENT` indicator bar; switching selection across `ProjectRoot`, `CardWorktree`, `ManagedWorktree`, or `Unassigned` immediately updates the tab strip without leaving tabs from the previous scope).
- **Simplified `Project -> Worktree -> Terminal` tree:** Only worktree scopes with at least one live Terminal appear in the left tree (idle `ManagedWorktree`s, idle `CardWorktree`s, and empty `ProjectRoot`s are omitted from the tree and remain accessible via the top-right Worktree Switcher, `+ [▾]`, and Quick Open). All active nodes render expanded without `▾/▸` collapse toggles. Projects with zero live Terminals render only a single `📁 ProjectName [⎇+] [+]` title row.
- **Git status indicators:** Active worktree branch rows and the top-right Worktree Switcher display green/red uncommitted line diff counts (`+N −M`, including untracked text files and `+0 −0` for non-text/mode-only dirty states) when the checkout has uncommitted changes (`TerminalContext.dirty`) and upstream tracking divergence (`↑N`, `↓M`) when an upstream tracking branch has commit differences (`ahead` / `behind`), omitting zero upstream counts. Context menus retain plain text indicators (`changed`).
- **Close fallback order (`Command+W` / `×`):** Closing the selected Terminal jumps first to the **previous** live Terminal in the **same `WorktreeScope`** (or **next** if none precedes it); if the worktree has no remaining live Terminals, it jumps to the **previous** live Terminal in the **same Project** in tree order (or **next** if none precedes it); if the Project has no remaining live Terminals, it jumps to the **previous** live Terminal **globally** in sidebar tree order (or **next** globally if none precedes it), clearing to the right-hand Empty State (`New terminal`) only when no live Terminals remain anywhere.
- **Creation & global `Command+T`:** Clicking top `+` (or pressing `Command+T` while on `Terminals`) inherits the current `WorktreeScope` (falling back to the active project's `ProjectRoot`). Triggering `Command+T` globally from non-`Terminals` pages (outside an open Card terminal split) opens a fresh Shell in the active project's `ProjectRoot` without inheriting prior sub-worktree sessions. Both UI worktree switching and shell `cd` dynamically update tree attribution and top tabs.
- **Unified real-time Herdr status & notifications (2026-11-15):** Every Terminal—Card Role Agents (`AgentRun`), ephemeral CLI Agents (`agent_engine`), Configuration/Merge sessions, and interactive Shells where a user manually launches an Agent CLI (`observed_agent`)—projects a single real-time `HerdrStatus` (`Working`, `Starting`, `Queued`, `Blocked` with `RuntimeWait` details, `Done`, `Idle`, `Error`, `Disconnected`, or quiet `PlainShell`). Top/Card tab strips display compact leading status marks (`live_dot(ACCENT)` for `Working`, amber shield/help icon for `Blocked`, green check for `Done`, danger triangle for `Error`/`Offline`, and quiet plain Shells without status clutter; Worktree headers carry no aggregate status counts), while Terminal sidebar rows place `[status_mark] + [status_word]` on the right end of their second line (keeping detailed wait/error reasons in the row hover tooltip rather than adding a third line). When a manual CLI Agent exits back to its login Shell (`zsh`/`bash`), AppServer clears any residual PTY OSC 7501 `agent_status`, `agent_turn`, and `waiting` fields. Active (`Working`/`Starting`) standalone/manual Agent terminals appear in `⌘I` `Running` without consuming Scheduler `max_agents` slots, while `Blocked`/`Error`/`Disconnected` terminals surface in `⌘I` `Needs you`, increment the top-right alert pill and primary `Terminals` rail badge, and trigger audible chimes via `sound_items`.
- **Compact fixed two-line sidebar session rows (2026-11-15):** Every session row in the `Terminals` sidebar tree (`crates/ui/src/terminals/tree.rs`) renders a uniform two-line compact layout (`h(px(46.))`) that never wraps or shifts height when selected, deselected, or blocked:
  - **Line 1 (pure title):** Displays only the session title, capped at 32 characters (`SIDEBAR_TITLE_MAX_CHARS = 32`, whitespace-collapsed with trailing `…`) plus single-line container `.truncate()` whether selected or unselected. Card stages (`Column`), Agent brand icons, and status indicators are omitted from Line 1 so full horizontal space belongs to `#N Card title…`, `<AgentEngine> N`, or `Terminal N`.
  - **Line 2 (left identity/machine · right Herdr status):** Left side renders `[Agent Icon] @role · Local` (or remote machine display name) for Card Role Agents—omitting redundant engine text names (`Antigravity`, `Codex`, `Claude`) and Card stages—`[Agent Icon] Local` (or remote machine) for standalone CLI Agents and Shells with an active observed Agent, and `[Terminal Icon] Local` (or remote machine) for plain Shells (`Local` is always retained). Right side renders the live Herdr status indicator (`● Working`, `🛡/? Blocked`, `✓ Done`, `ⓘ Needs you`, `● Idle`, `● Starting`, `● Queued`, `● Closing`, `▲ Error`, `▲ Offline`), leaving normal idle plain Shells blank on the right. Full untruncated titles, original `#N Card title` on renamed Agents, stage/engine details, and `Blocked`/`Error` reasons remain available in the row hover tooltip (`sidebar_hint`).

## Shared navigation and execution boundaries

The Project-grouped inventory lists both automatically scheduled Agent conversations and manually opened Shells. Tabs are currently opened views of those entries: at most one tab per entry, including across Projects. Clicking an entry or opening it from Card, Overview, Notes or Quick Open selects its existing tab or adds one. Background Agent scheduling never steals focus or opens a foreground tab.

Managed Agent sidebar entries render `#42 Card title…` (capped at 32 characters, single-line truncated) on Line 1, and `[provider icon] @profile · Local` (or remote machine name) on the left of Line 2 with the Herdr status mark + word on the right—omitting redundant provider text names (`Antigravity`, `Codex`, `Claude`), Card stage labels, selection wrapping, and third-line wait/error subcopy so row height stays fixed at two lines. Full Card title, stage, provider name, original `#N Card title` (when renamed), and wait/error reasons appear in the row hover tooltip. On top/Card tabs, the stage belongs to the selected Request; moving a Card does not relabel an older Agent's task as the new stage. A recognized launch executable determines the brand mark, otherwise a declared preset supplies the presentation hint (`Icon::Agent` fallback for custom providers). This never selects runtime behavior.

Agent tabs remain single-line: `#42 Card title… · Develop · [provider icon] developer`. Card titles get flexible space and trailing ellipsis. Stage/Profile widths are reserved, tabs have bounded widths fitted to the strip, and the strip scrolls horizontally. Header space is explicitly budgeted between tabs, context and controls, keeping selected-tab close, New terminal and actions reachable. Hover/accessibility text contains the full Card title, Project, stage, provider/Profile and runtime context.

Independent Terminals default to **Terminal 1**, **Terminal 2**, etc. (or `<AgentEngine> 1`, `<AgentEngine> 2` for standalone interactive Agent CLIs). In the sidebar tree, Line 1 shows only the single-line title (`Terminal N`, `<AgentEngine> N`, or custom name, capped at 32 characters), while Line 2 shows `[Terminal Icon] Local` (or remote machine name) for plain Shells and switches its leading mark to `[Agent Icon] Local` for standalone Agent CLIs or while Herdr observes a recognizable foreground Agent CLI inside a live connected Shell, paired with the live Herdr status on the right. On top tabs, plain Shells have no generic shell icon while standalone/observed Agent CLIs display their Agent mark before the title. Default numbering is computed dynamically per Project from currently live independent Terminals (`max(N) + 1` among live `<Prefix> N` titles in the same Project, or `<Prefix> 1` when none remain), without a durable manual counter: Agent creation/resume does not consume it, closing an earlier Terminal does not rename surviving entries, and closing all live Terminals in a Project resets the next Terminal back to `Terminal 1`. Custom names override the title for both kinds; double-click a tab or use the actions menu to rename. Clearing the custom name on an independent Terminal regenerates the next live `<Prefix> N` label in its Project. Closing a tab retains its saved entry/history and terminates its live PTY. The native window title exposes the same context.

`+` / Command+T immediately opens the user's Shell without requiring a Project or a naming confirmation. It inherits the selected Terminal's machine and current directory from Herdr; without a selection it uses the configured default machine and its home. The machine menu explicitly opens a Shell on another host. Board selection does not supply an implicit owner. Project `+` opens in that Project directory; Card's Open Shell action uses its existing worktree, otherwise its Project directory. These are launch conveniences, not permanent Shell associations. Missing explicit directories are actionable failures; no worktree, fetch, merge or workflow action is implicit.

The left inventory groups managed Agents by their task Project, independent Terminals by their observed directory's Project, and unmatched entries under **Other**. There is no nested Card hierarchy or separate source filter. Regrouping reveals the selected row without changing tab order, selection, Board selection or the mounted native viewer. New Projects are never created by directory detection.

The right chrome shows a clickable task Card when present, otherwise a weakly inferred Card, otherwise a clickable Project or compact directory. An inferred Card uses muted text and a tooltip explaining the directory/branch match. Branch always comes from the observed checkout. Long branch/directory values use middle ellipsis with a hover showing their full values; copy/menu actions retain the original strings. Clicking Card (or choosing **Open Card** from the Terminal / Card worktree menu) opens the Card workspace with its right-hand Terminal split expanded and focused on that Terminal (forming a symmetric round-trip with `[↗]` Pop out). Full directory, profile, runtime controls and history live in the actions menu or explicit details view. Herdr unavailability clears inferred context; a task Card remains because it is durable product context.

## Identity and persistence

```text
TerminalEntry (stable ID / ordering number / origin_project_id? / name)
    |
    +-- conversation_id? -> Conversation -> Project/Card/Profile
    |                           +-> Request/Attempt -> owning Core/PTy
    |
    +-- runtime? -> independent Terminal's owning Core/PTy + lifecycle receipt
                         |
                    Herdr observed cwd
                         |
                   Project / Git / Card?   (ephemeral hints)
```

SQLite schema v5 migrates v4 Terminal entries transactionally, preserving IDs, names, numbers and Core/PTy bindings. `TerminalSource`, permanent Shell Project/Card ownership and stored Shell cwd are removed. Agent bindings remain on Attempts; `conversation_id` expresses the optional task association. Existing operation receipts are decoded compatibly when read and are not rewritten. Manual Terminals create no Card, Request, Attempt or workflow outcome.

Launch cwd is only an API argument. Live cwd comes from nonempty `foreground.cwd`, falling back to `terminal.cwd`, matching the Herdr UI client. It is an observation of the foreground process, not a guaranteed shell-prompt directory. AppServer consumes one Core event subscription with an initial snapshot/reconnect recovery and publishes in-memory, Core/PTY-fenced `terminal_contexts` in snapshots. Directory/Git discovery is bounded and cached; there is no periodic inventory/status scan. See [runtime observation](runtime-observation.md). Contexts are recomputed after restart and disappear when unavailable. `Attempt.cwd` remains historical execution context, never a substitute for live location.

Project matching uses canonical path boundaries, the most specific registered directory, known Card worktrees, or an unambiguous common Git repository. Card hints require one known active worktree match or one active Card matching both repository and branch; ambiguous matches remain unassigned. A task Card always takes precedence for managed Agents, even if their foreground directory belongs to another Project. Hints never authorize reporting, resuming, workflow advancement or merging.

Schema v6 originally added a separate `manual_number` and counter; Schema v18 removes `manual_number` and the `'terminal-manual-number'` counter, materializing prior automatic labels onto `TerminalEntry.name` and switching new independent Terminal titles to per-Project live `max(N) + 1` allocation (`origin_project_id` records the initial Project scope before live `terminal_contexts` observation). Custom names, IDs, ordering numbers, Core/PTy bindings and Agent records remain intact. Historical operation receipts are not rewritten.

Names and runtime identities survive AppServer/UI restart; opened tabs, selection and fold state remain process-local UI state. Core history is bounded, not complete durable terminal scrollback.

Only the selected eligible live entry binds the mounted libghostty-vt `NSView`, while a bounded in-memory LRU pool of warm `LiveSession` streams preserves parsed VT screens, scrollback, and live Herdr attachments across tab switches and Card-to-Terminals transitions. Switching between live tabs rebinds the existing native viewer in place without tearing down or recreating the `NSView`. Wide sidebar visibility changes preserve that viewer and focus. Narrow full-width tree viewing detaches its hidden viewer, never the runtime. Missing/replaced Core cannot cause rebinding to a new PTY. Disconnected input is disabled and never replayed.

## Lifecycle and controls

- Agent Resume explicitly uses its provider profile and expected historical attempt, retaining the Card/profile identity and original environment. Browsing queued/history/archived context never starts work. Finished entries are absent from the live Terminal inventory/tabs; Card and Overview provide saved history and explicit Resume. Existing human gates and Scheduler admission remain authoritative.
- A finished Shell can be **Reopened** as a new entry/PTy using its Herdr-observed directory, falling back to home if unavailable; this is not process resume. Unknown launch outcomes remain visible and are never automatically replayed. Launch intent is durable before network IO; request keys prevent duplicate manual launches.
- Closing a tab using X, Command+W or Close terminal ends its associated live Herdr PTY and removes the tab. When a live Agent is running in that Terminal (managed Card/Workflow Agent, Project Workflow configuration Agent, ephemeral CLI Agent, or a Shell with an active OSC-7501 Agent) and its connected status is not `done` or `idle`, the UI first opens a confirmation dialog warning that closing terminates the live process; Agents in `done` or `idle` and plain Shells without an active Agent close immediately. Once confirmed, the UI optimistically removes the tab and switches selection in 0 ms without blocking other Terminal actions while `terminal.close` completes in the background. All confirmed close triggers call the same AppServer `terminal.close` lifecycle operation. AppServer resolves the exact owning Agent Attempt or independent Terminal binding and persists the close intent before runtime IO. Existing original-Core checks remain authoritative. Failed or uncertain termination rolls the tab back into place and displays an error. Finished entries leave the Terminal list/tabs; saved Agent history and explicit Resume remain available through Card/Overview. Hiding the sidebar, leaving the page or quitting the UI only detaches the viewer and leaves runtime processes running.
- Manual Shells do not consume automatic Agent slots or inherit Agent idle reclamation. Running an Agent command manually in a Shell does not enroll it in Workflow or automatically move a Card.
- Manual Shell state is observed separately from Agent lifecycle, including original-Core checks. Archiving a Card retains its worktree if any independent Terminal is observed inside that worktree, regardless of its creation origin; unresolved live Terminal locations conservatively retain the checkout, and records the retention reason.
- Command+T creates, Command+W terminates the current Terminal and closes its tab, Command+1..9 jumps to the 1st through 9th tab in the current workspace tab strip, and Command+Shift+[ / ] cycles tabs while the Terminal workspace owns focus. Rename and Quick Open temporarily suspend Terminal shortcuts. Closing the active tab selects a fallback tab (`WorktreeScope -> Project -> Global`); closing the last live Terminal leaves a direct New terminal action.

## Implementation ownership

- `types::TerminalEntry` and storage: presentation identity, numeric allocation and schema migration.
- `app-server/terminals.rs`: manual Shell intent/receipt, rename/control and unified close; no Scheduler admission for Shells. `observation.rs` owns the shared runtime inventory, and `directory.rs` owns bounded cached context discovery.
- `ui/terminals/entries.rs`: common Agent/Shell projection and binding eligibility.
- `ui/terminals.rs`, `tree.rs`, `tabs.rs`, `viewer.rs`: navigation, split chrome, live inventory, open tabs, secondary actions, rename and single selected native viewer.
- `ui-kit/chrome.rs`: generic compact icon controls shared across production and Gallery.

## Previous split-chrome acceptance (before weak association)

All **71 Rust tests**, workspace/all-target Clippy (`-D warnings`), app/Gallery builds, formatting, Python syntax and whitespace checks passed. Tests cover v3-to-v4 conversation migration, stable numeric names, manual launch/control idempotency, original-Core fencing, missing directories, unknown launch receipts, Scheduler separation, Agent resume projection and valid NSH shortcut names.

Native acceptance used `/private/tmp/cb-terminals-sa_hnir1`: three disposable Projects, actual Herdr PTYs, real login Shells and deterministic interactive Agent fixtures. This did not run paid/live Codex or Claude tasks or touch the user's Core/home.

- The full-height divider resized from 316 to 390 points and moved the titlebar boundary with it. Card/branch stay in chrome, with no extra Terminal title/toolbar/footer in the content area. Gallery checked a 740-point dark window and a wide light window, native VT rendering, multiple profiles, archived restrictions and numeric names. Narrow inventory fills the actual viewport; its Project `+` stays reachable.
- Card Agent, direct tree selection, restored rail selection and Quick Open reach existing bindings. Distinct Agent input reached its owning PTY. Cross-Project Back restored the original Alpha Card and unsent comment draft. Opening queued/closed entries added no Requests/Attempts.
- `+` and Command+T opened Shells immediately without a naming dialog. Native focus worked after mount/switch and returned after rename/search. Double-click selected the old name for replacement. At that earlier acceptance, Command+W closed only the view; this behavior is superseded by the 2026-11-09 close-and-terminate decision below. Wide sidebar hiding preserved the viewer and typing. Keyboard tab cycling worked.
- UI restart retained numeric/custom names and Core/PTy bindings without adding execution IDs. Exact Agent Resume kept entry/tab `1` and its provider conversation ID, creating a new Request/Attempt/PTy. Manual Shells launched while the automatic Agent budget was full; they did not consume its slots or advance Cards.
- A third Project used a real separate Git worktree. Its Shell's `pwd` and titlebar branch matched that checkout. Changing a branch inside another Shell updated the titlebar asynchronously. Archiving the worktree Card while its Shell remained live retained the checkout and recorded why.
- A scratch transport wrapper simulated unavailability with Core/PTys alive. The native viewer displayed input-disabled status. An offline command that would create `offline-leak.txt` was dropped, the same Shell reconnected, and the file remained absent after recovery. This is transport-unavailability acceptance, not a destructive Core-crash test.
- Interrupt returned the selected Shell's foreground command to its prompt. Explicit End closed only the selected Shell; other Agents/Shells stayed alive. Reopen created the next numbered entry. Native menu End used a pointer fallback: CUA's AX-driven native-menu item activation was unreliable, so this is not full menu accessibility acceptance.
- A fast Command+O/type sequence reproduced a pre-fix search-prefix leak. The final native command-key guard synchronously suspends PTY text delivery while the UI transfers focus, including pending tab creation. `input-guard.json` confirms `INPUT after-rapid-search-guard` with no `RAPID_SEARCH` prefix in the PTY. Keys arriving before the search field is ready are suppressed; they are not buffered/replayed to the PTY.
- Runtime refresh previously disposed a transient button's in-flight task after a Shell was created, leaving the UI pending. Commands now belong to the window owner, reset pending before callbacks, and retain source fences; stale refresh snapshots cannot overwrite a newer command receipt. Final UI creation automatically selected the new Shell. Creating another Shell and immediately navigating to Board preserved its receipt, released the UI and did not redirect the user. Re-entering Terminals retained the prior selected tab.

The task-owned app/Gallery exited normally and the harness stopped its isolated Core (its socket was removed). The user's running app/Core were left alone.

Final evidence records **11 Terminal entries, six Agent Requests and six Attempts**: additions beyond the initial Agent fixture were explicit Resume and the separate-worktree fixture. Manual Shells and tab navigation generated no Agent execution. Evidence files are `native-checks.json`, `before.json`, `after-tabs.json`, `before-restart.json`, `after-restart.json`, `before-resume.json`, `after-resume.json`, `worktree.json`, `input-guard.json` and `final-runtime.json` in the scratch directory.

Chinese IME remains excluded at the user's request. Open tabs/selection/folds remain process-local; durable names/bindings and bounded Core history are retained. External/unassociated Herdr workspaces and multi-host navigation remain outside this slice. The earlier acceptance below describes the preceding conversation-only implementation.

## Earlier conversation-workspace acceptance

All **65 Rust tests**, all-target Clippy (`-D warnings`), application/Gallery builds, formatting and whitespace checks passed. New model regressions exercise identity and binding boundaries rather than duplicating UI layout assertions.

`mise run acceptance-terminals` prepares the isolated packaged app, scratch Core and deterministic interactive providers for native/manual checks. It does not run paid/live Codex or Claude tasks. The exercised run is `/private/tmp/cb-terminals-6sr745ox`:

- Initial state: two Projects, two live Agent PTYs, one queued conversation, one closed provider conversation. Card Open terminal reused the existing execution and opened Terminals.
- Sent distinct Alpha/Beta messages through the actual native Terminal. The input audit confirms each message reached only its owning PTY. Hiding/showing the wide sidebar retained the native AX element, content and focus; subsequent input worked without refocusing.
- After cross-Project viewing, Back returned to the original Alpha Card and preserved its unsent comment draft. Direct rail entry restored the last selected Beta conversation. Open Card and Close Card/Back return were checked with the production Gallery components.
- Browsing live/queued/closed conversations added **no requests or attempts** and preserved human gates: `before.json` versus `after-navigation.json` retains four request IDs and three attempt IDs.
- The scratch execution budget was then raised explicitly to allow the queued fixture to start and leave room for a manual Resume. Exact Resume created a new request/PTY with the **same provider session ID**, retaining one conversation row. `after-resume.json` records five requests/attempts, including historical ones.
- Overview opened the exact same Alpha PTY with an Overview return; Quick Open from a focused native Terminal navigated to Beta's existing conversation. Search text never entered an Agent input audit.
- The harness's transport wrapper simulated unavailability while its scratch Core/PTys remained alive. Reattachment showed the original conversation with input disabled. Restoring transport reattached the same Core/PTY; offline keystrokes were absent from the input audit and were not replayed. This is transport-unavailability acceptance, not a destructive Core crash/host-loss test.
- Interrupt produced the selected Beta fixture's `INTERRUPTED` output. End session closed Beta's PTY and retained its conversation; Alpha and other Agents remained live. No extra request/attempt appeared from these navigation/control operations.
- Gallery checked multiple profiles on one Card, queued waiting, archived read-only context without Resume, Closed expansion, Project folding, sidebar resize from 260 to 316 points, retained width, 720-point narrow list/viewer switching, single Card-return control, and light/dark appearance. Simulated live Gallery rows use the production native renderer with static content and never connect to a Core.

Evidence: `before.json`, `after-navigation.json`, `after-resume.json`, `final.json`, `input-audit.json` and `native-checks.json` in the scratch directory. The task-owned app/Gallery/Core were stopped after verification. Chinese IME remains excluded at the user's request. UI selection/folds/source history are process-local; full terminal scrollback persistence, external/unassociated Herdr workspaces and multi-host navigation remain outside this change.

## Independent-context acceptance (2026-11-08)

All **78 Rust tests**, workspace/all-target Clippy (`-D warnings`), app/Gallery builds, formatting and whitespace checks passed. `mise run acceptance` passed the real Herdr fixture workflow, report/archive/restore, WAL, restart and multi-Board scheduling checks. New regressions cover v4 migration and immutable historical receipts, no-Project creation, observed-cwd inheritance, Core/PTy fencing, directory boundary/symlink/worktree/branch ambiguity, durable Agent task ownership, worktree retention and read-only native input/status.

Native CUA acceptance used `/private/tmp/cb-terminal-context-hgcqbvyq`, a copied app, isolated Canban home/Core, real login Shells and a deterministic interactive Agent. No live paid Codex/Claude task or user Core was used. Evidence: `zero-project.json`, `worktree-context.json`, `after-resume.json`, `final-runtime.json`, `native-checks.json` and `cleanup.json` under that directory.

- Created Terminal 1 through the UI with zero Projects; no picker was required. Entering a Project subdirectory containing spaces regrouped it under Alpha and showed the observed branch. Typing continued on the same PTY.
- Command+T inherited that observed subdirectory. Entering the separate Card worktree added a muted inferred Card link without creating a Request. Following Card and returning retained the same Shell. Moving to Beta changed presentation context without changing Terminal identity.
- Fixed Card-return focus: a fresh explicit focus ticket now lets the managed Agent receive `AFTER_CARD_RETURN` without another click. Repeated native app restart retained Core/PTy identities and numeric names; directory groups were reconstructed from Herdr.
- A scratch wrapper disabled control RPC while an existing attach stream stayed alive. This reproduced input continuing despite unavailable observation. The final fix gates native input using AppServer connection state as well as stream state, displays `Read-only · input disabled`, drops offline keys and resumes fresh input on the same PTY. `offline-protected` remained absent after reconnection. The earlier `offline-leak` file is evidence of the pre-fix failure, not a final passing result.
- Ended and resumed Agent 2 in the UI: entry number and provider session stayed unchanged, a new PTY was created, and `RESUME_NATIVE_OK` reached that resumed Agent. Independent Terminals did not add Requests/Attempts.
- The narrow 740-point dark Gallery reproduced long Card text hiding branch/actions. Flexible constrained text and reserved control widths fixed it; Card/branch now truncate while the actions menu remains visible. Gallery includes both Other and inferred-Card independent Terminals. Existing fast Quick Open prefix suppression and bounded Core history limitations still apply.

`mise run acceptance-terminal-context` prepares an empty isolated native workspace for repeating these checks. Desktop apps require an execution environment with AppKit window access; the sandboxed launch exited without creating a window, so native acceptance used an authorized unsandboxed launch of the disposable fixture.

## Two-line Agent labels (2026-11-08)

This acceptance introduced Card/stage/Profile titles in place of numeric Agent labels. At that time, production `terminals/labels.rs` shared Provider/Profile rendering between two-line inventory rows and single-line tabs; the later [inline metadata contract](#inline-session-metadata-2026-11-09-superseded-in-part-by-2026-11-15-compact-two-line-sidebar-rows-above) adds machine names and separates list/tab tooltip behavior. UI Kit owns the licensed provider SVGs. Presentation tests cover execution-stage ownership, auto/custom names, executable/preset marks and generic fallback. Storage tests cover v5-to-v6 backfill, independent numbering among Agent entries, immutable runtime bindings and clearing a custom name.

All **80 Rust tests**, workspace/all-target Clippy (`-D warnings`), app/Gallery builds, fmt and whitespace checks passed. `mise run acceptance` passed the isolated real-Herdr workflow, archive/restore, restart and multi-Board checks (`/private/tmp/cb-mltmm8_8`).

Native acceptance used `/private/tmp/cb-terminals-bu7qw2bz` with two Projects, deterministic interactive Agents and real login Shells on a scratch Core. Agent entries preceded the first two independent Terminals, which still displayed Terminal 1 and Terminal 2. Renaming Terminal 1 to Build shell, restarting the app and clearing the custom name preserved its number and Core/PTy. Updating a live Card to a long mixed Chinese/English title changed its row/Tab title and still delivered `TITLE_CHANGED_INPUT_OK` to the original Agent. Request counts and runtime identities stayed unchanged. Evidence: `labels-before-restart.json`, `labels-final.json`, `label-checks.json`, `labels-cleanup.json`. Both the disposable app and Core were stopped; the user's app/Core were untouched.

Gallery checks covered 740-point dark and 1120-point light layouts, Codex/Claude marks, long Unicode Card names, queued/working/attention states and numbered independent entries. The checks exposed and fixed undersized standard-stage labels, header overcommit when long tabs were added, and Card-number loss when a narrow tab truncated its title. A narrow tab retains the Card number/stage/provider/Profile and close control, with title details available in chrome/hover. Full branch/directory tooltip values were checked in the component source and accessibility data; long branch shortening was checked visually in the real app. These checks used fixtures, not live paid Codex/Claude sessions.

## First-Terminal regression (2026-11-09)

After multi-machine integration, opening Terminals with no selected entry and
clicking New terminal sent `inherit_terminal_id: ""`. AppServer attempted to
resolve that empty ID and returned `Terminal not found` before reaching Core.
The UI now omits inheritance when nothing is selected; AppServer also accepts
an empty legacy inheritance ID as absent. A nonempty selected ID still follows
the existing machine/live-cwd rules and does not silently fall back.

A regression test reproduces the exact empty-string request in a fresh home
without Projects, conversations or Terminals. It failed before the fix and
passed afterward; 89 workspace Rust tests and Clippy passed. In an isolated
native app backed by a real Core, the empty-state New terminal button opened
Terminal 1 and accepted shell input. Closing the last tab and pressing Command+T
opened Terminal 2; another Command+T after changing cwd opened Terminal 3 in
that observed directory. These checks created no scheduled Agent work.

## Post-Archive Agent Resume (2026-11-09)

Real model acceptance found that Restore could leave a historical Agent's original managed worktree absent. Resume now reconstructs that exact checkout from retained Card Git refs on its original machine. It never substitutes the Project's new location or a fresh base. Missing unmanaged directories or lost refs still require inspection. A real Codex session was restored after Archive, received a native Terminal question, recalled its original context and completed/closed normally; code and Notes remained unchanged. See [real Agent acceptance](real-agent-acceptance-20261109.md).


## Close terminates the Terminal (2026-11-09)

User-selected update: the tab X, Command+W and context-menu Close terminal share termination semantics for live Agent and independent Shell tabs. UI sends only the close intent; AppServer owns termination and a durable close receipt. Wait for a successful close receipt before removing the tab; retain it on failure or uncertain delivery. Selecting the next neighboring tab and the last-tab New terminal empty state remain unchanged. Closing already finished/history or not-yet-launched entries removes only the tab because there is no live PTY to terminate. This action does not delete provider history, accept a Review or report task completion. Existing Resume/Reopen actions retain their separate meanings.


### Explicit connection and closing labels

The native viewer and inventory show Connecting, Disconnected or Closing as the reason input is gated. `read_only` remains an internal input-control flag, not a user-facing state. The native stream owns its attach/reconnect status; the product binding supplies closing or disconnected observation state without changing the immutable Core/PTy identity. Ended entries retain their Resume/Reopen actions. Static Gallery Terminals show Preview.


Close lifecycle acceptance: real isolated Core/PTys at `/private/tmp/cb-close-000lb28a` passed Shell close with neighboring PTY isolation, Agent close without workflow completion, same-provider Resume, old-receipt replay after Resume and unavailable-Core/explicit retry. 108 workspace tests passed. Native automation access to the fresh cloned acceptance app was not approved in this run; the close-button click and new state-label layout are not claimed as native interaction acceptance.

## Compact Card entry (2026-11-09)

The [Card conversation](card-conversation.md) header/Timeline link into the existing Terminal workspace. Navigation alone never starts/resumes a process. A Card without a live conversation opens a compact menu with saved history and eligible explicit Start/Resume choices; it does not open an empty Terminal page. Existing terminal menus and ended-session controls own lifecycle management and history. Back to source restores the Card's draft and scroll offset.

## Inline session metadata (2026-11-09; superseded in part by 2026-11-15 compact two-line sidebar rows above)

The middle session list presents its metadata directly, with no row-wide or nested Provider tooltip. Agent rows show Card/custom title and stage, then Provider/Profile and state, then execution-machine name. Identical Provider/Profile names collapse to one label. Manual Shells show their name and state followed by machine name; they have no generic Terminal-type icon, displaying an Agent mark only while Herdr observes a recognized Agent CLI running in that Shell. Local execution is labelled **Local**; remote execution uses the machine catalog's display name.

Unselected titles remain compact. Selecting a row wraps the complete title and Provider/Profile text; a renamed Agent also reveals its original Card number/title. The selected fill remains stable while hovered, and keyboard focus remains visible. Current directories are absent from both visual rows and their accessibility labels. Project grouping supplies Project context, and branch remains in the selected Terminal chrome. Compact top tabs, branch hover and secondary action menus retain their existing contracts.

This is a presentation change only: Terminal identity, activation, admission, input, Resume and directory association are unchanged. Cwd-only changes within the same association no longer invalidate the list's display signature. Gallery fixtures cover a remote Agent, a differently named Claude reviewer, a long custom remote Shell, Local/remote names and disconnected state. Check title expand/collapse, list scrolling and hover without an information popup, then verify actual session switching and input using an isolated production App.

Executed acceptance: wide and narrow native Gallery layouts passed title expansion, Provider/Profile deduplication, Local/remote machine labels, disconnected Shell selection, scrolling and hover without a metadata popup. Long Project headings truncate without obscuring New terminal. An isolated production App with a real Core and deterministic Agent passed Agent/Shell switching, native input, state updates and retained output; current directories were absent from session rows. Evidence is under `/private/tmp/cb-terminal-sidebar-native-203neuzo`. Build, Clippy and all 136 workspace tests passed.

## Project Workflow configuration Agents

Project-owned configuration Agents are an additional stable owner, independent of Card assignments or inferred directories. The tree groups these entries under **Project / Configuration / Workflows · Profile**. They reuse the native viewer and terminal lifecycle controls; **Back to Workflows** returns to the Project editor. **Edit with agent** directly starts, continues, or resumes the configured default Agent's configuration Terminal without a secondary profile picker. Continue only focuses a retained binding; Resume uses exact provider history (falling back to a fresh conversation when exact history or resume support is unavailable). Closing a queued entry cancels admission. See [the configuration contract](product/workflow-engine.md#edit-with-agent).

## Standalone Managed Branch Worktrees (2026-11-13)

Users frequently want to spin up an isolated Git branch worktree inside a Project for ad-hoc exploration, direct coding, or interactive CLI Agent work without first authoring a Board Card. Canban manages these standalone checkouts as durable `ManagedWorktree` records (SQLite schema v13, `Snapshot.worktrees`, `DESKTOP_API` 19+):

- **Creation (`worktree.create`):** Triggered from a Project's `+` menu in `Terminals`, the top `+` menu, or Command Palette (`New Managed Branch Worktree…`). Users choose the Project, enter a branch name (validated by `git check-ref-format --branch`), optionally pick a base branch and upstream sync, and choose what to open immediately: a login **Shell** in the new worktree or a configured **Agent CLI** command running interactively in that directory. Worktrees are allocated under `<execution-home>/worktrees/<namespace>/<leaf>` on the Project's current execution machine (local or remote SSH) using the same collision-safe `canban-workspaces` allocator as Card worktrees.
- **Retention across tab closes:** Closing the last Terminal tab inside a `ManagedWorktree` (`Command+W` / tab `X`) terminates only that PTY tab (`terminal.close`) and **retains the `ManagedWorktree`** in the Project's `Terminals` sidebar tree (`⎇ <branch>`). Users can reopen a Shell or Agent CLI inside it at any time with one click, without losing uncommitted edits or local commits.
- **Diff, Promote to Card, and Delete:**
  - **Open in Diff & Review:** Opens `Diff & Review` comparing the worktree's branch against its `base_branch`.
  - **Promote to Card (`worktree.promote`):** Creates a new Board `Card` in the owning Project bound directly to the existing `branch`, `path`, `base_commit`, and `location`, atomically transferring ownership from `ManagedWorktree` to the new `Card` without moving files or interrupting open Shells.
  - **Delete Worktree (`worktree.remove`):** Refuses removal while any live Terminal PTY is observed inside the worktree. Checks `git status --porcelain` (`worktree.status`); clean worktrees remove immediately (`git worktree remove`), while dirty worktrees surface a warning requiring explicit `force: true` confirmation before discarding uncommitted changes and deleting the `ManagedWorktree` record.

## Unified Terminal navigation

Updated 2026-11-11. Overview, Card header/profile controls, Board Terminal buttons/context menus, Review, Timeline, Quick Open and Terminal controls converge on shared target resolution in `terminals/navigation.rs` and `Ui::open_terminal`.

An exact live target reuses its tab, reveals its tree group and focuses the existing PTY. Disconnection retains the original binding and read-only connection feedback. Missing targets produce an explicit error. A queued request without a PTY opens its saved Card request context; navigation cannot bypass admission or create a replacement.

For a Card-level entry, reuse its last explicitly opened live Agent, otherwise open the sole live Agent. Multiple candidates show a native menu. With no live target the menu offers saved request/history, eligible Resume Agent and explicitly labelled Start choices for profiles without an existing conversation. Archived Cards cannot start execution. A direct profile link skips the picker.

Timeline request links attach only when that request is the current live invocation. Historical requests open their own assignment/state/output on the Card, with a separately labelled Open current conversation action when applicable. Closed Quick Open candidates advertise View history. Resume is an explicit exact-session operation with an expected-attempt fence; unavailable provider history cannot silently become a fresh start. Start and Resume receipts respect pending guards. When admission initially queues a request, the UI shows its context and attaches only after that exact request obtains a PTY. Leaving that context cancels automatic navigation. Successful attachment returns directly to the launch origin on Back, without an extra queue-context history entry.

Back/Forward retain existing route, draft and scroll behavior and never replay execution. Overview no longer embeds a Terminal preview or context inspector. The [entry inventory and design](overview-and-terminal-navigation-proposal.md) records the full cross-surface scope.
