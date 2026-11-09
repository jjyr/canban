# Dedicated Terminal workspace

Multi-machine update, 2026-11-09: see [current execution/location and service contract](multi-machine.md). Profile commands resolve on their execution host; Resume and Review retain their original workspace, while new work uses the Project's current location.


Implemented user-approved direction, 2026-11-08. Native/runtime acceptance is recorded below. This supersedes the Terminal portion of the earlier Card-focused split layout. Review retains its own delivery interface.

## Product layout (updated user-approved direction)

```text
+-----------------------------+----------------------------------------------+
| traffic lights  [Back]       | [1 x] [2 x] [+]        #42 Card / branch [...] |
|                             +----------------------------------------------+
| rail | Project groups       |                                              |
|      | v Canban         [+] |             Native Terminal                  |
|      |   1                  |                                              |
|      | v Other              |                                              |
|      |   2                  |                                              |
+-----------------------------+----------------------------------------------+
```

The window chrome follows the same left/right divider as the content. The left panel owns the native traffic-light clearance, source navigation and sidebar controls; the right panel owns tabs, current Card and branch, and a compact actions menu. Resizing the left panel moves its full-height divider. With the list hidden, tab controls clear the native window buttons. NSH owns window dragging, system double-click behavior, traffic-light geometry and resizable panels. Terminal content has no separate title, toolbar, tab strip, placeholder asking to select a conversation, or permanent technical status footer.

## Shared navigation and execution boundaries

The Project-grouped inventory lists both automatically scheduled Agent conversations and manually opened Shells. Tabs are currently opened views of those entries: at most one tab per entry, including across Projects. Clicking an entry or opening it from Card, Overview, Notes or Quick Open selects its existing tab or adds one. Background Agent scheduling never steals focus or opens a foreground tab.

Managed Agent entries use two lines in the inventory: `#42 Card title…` with the execution stage reserved on the right, then `[provider icon] profile` with the runtime status on the right. The stage belongs to the selected Request; moving a Card does not relabel an older Agent's task as the new stage. Codex/Claude marks sit directly before the Profile name; custom providers use the generic Agent mark. A recognized launch executable determines the mark, otherwise a declared preset supplies the presentation hint. This never selects runtime behavior.

Agent tabs remain single-line: `#42 Card title… · Develop · [provider icon] developer`. Card titles get flexible space and trailing ellipsis. Stage/Profile widths are reserved, tabs have bounded widths fitted to the strip, and the strip scrolls horizontally. Header space is explicitly budgeted between tabs, context and controls, keeping selected-tab close, New terminal and actions reachable. Hover/accessibility text contains the full Card title, Project, stage, provider/Profile and runtime context.

Independent Terminals have no type icon and default to **Terminal 1**, **Terminal 2**, etc. Their durable sequence is separate from global entry ordering: Agent creation/resume does not consume it, and closing a view does not renumber it. A small status dot still indicates runtime state. Custom names override the automatic title portion for both kinds; Agent stage/Profile remain visible. Double-click or use the actions menu to rename. The optional field starts empty for automatic names; clearing it restores the automatic title. Closing a tab retains its saved entry/history and terminates its live PTY. The native window title exposes the same context.

`+` / Command+T immediately opens the user's Shell without requiring a Project or a naming confirmation. It inherits the selected Terminal's current directory from Herdr, otherwise starts at the user's home directory. Board selection does not supply an implicit owner. Project `+` opens in that Project directory; Card's Open Shell action uses its existing worktree, otherwise its Project directory. These are launch conveniences, not permanent Shell associations. Missing explicit directories are actionable failures; no worktree, fetch, merge or workflow action is implicit.

The left inventory groups managed Agents by their task Project, independent Terminals by their observed directory's Project, and unmatched entries under **Other**. There is no nested Card hierarchy or separate source filter. Regrouping reveals the selected row without changing tab order, selection, Board selection or the mounted native viewer. New Projects are never created by directory detection.

The right chrome shows a clickable task Card when present, otherwise a weakly inferred Card, otherwise a clickable Project or compact directory. An inferred Card uses muted text and a tooltip explaining the directory/branch match. Branch always comes from the observed checkout. Long branch/directory values use middle ellipsis with a hover showing their full values; copy/menu actions retain the original strings. Clicking Card opens its detail with a Terminal return action. Full directory, profile, runtime controls and history live in the actions menu or explicit details view. Herdr unavailability clears inferred context; a task Card remains because it is durable product context.

## Identity and persistence

```text
TerminalEntry (stable ID / ordering number / optional manual number / custom name)
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

Launch cwd is only an API argument. Live cwd comes from nonempty `foreground.cwd`, falling back to `terminal.cwd`, matching the Herdr UI client. It is an observation of the foreground process, not a guaranteed shell-prompt directory. AppServer observes `terminal.list(all=true)` in its background runtime loop and publishes in-memory, Core/PTy-fenced `terminal_contexts` in snapshots. Contexts are recomputed after restart and disappear when unavailable. `Attempt.cwd` remains historical execution context, never a substitute for live location.

Project matching uses canonical path boundaries, the most specific registered directory, known Card worktrees, or an unambiguous common Git repository. Card hints require one known active worktree match or one active Card matching both repository and branch; ambiguous matches remain unassigned. A task Card always takes precedence for managed Agents, even if their foreground directory belongs to another Project. Hints never authorize reporting, resuming, workflow advancement or merging.

Schema v6 adds a separate `manual_number` and counter. Existing independent entries receive numbers in their established order; custom names, IDs, ordering numbers, Core/PTy bindings and Agent records remain intact. Historical operation receipts are not rewritten.

Names and runtime identities survive AppServer/UI restart; opened tabs, selection and fold state remain process-local UI state. Core history is bounded, not complete durable terminal scrollback.

Only the selected eligible live entry mounts the existing libghostty-vt component and Herdr stream. Wide sidebar visibility changes preserve that viewer and focus. Narrow full-width tree viewing detaches its hidden viewer, never the runtime. Missing/replaced Core cannot cause rebinding to a new PTY. Disconnected input is disabled and never replayed.

## Lifecycle and controls

- Agent Resume explicitly uses its provider profile and expected historical attempt, retaining the Card/profile identity. Selecting a queued/closed/archived entry never starts work. Existing human gates and Scheduler admission remain authoritative.
- A finished Shell can be **Reopened** as a new entry/PTy using its Herdr-observed directory, falling back to home if unavailable; this is not process resume. Unknown launch outcomes remain visible and are never automatically replayed. Launch intent is durable before network IO; request keys prevent duplicate manual launches.
- Closing a tab using X, Command+W or Close terminal ends its associated live Herdr PTY before removing the tab. All close triggers call the same AppServer `terminal.close` lifecycle operation. AppServer resolves the exact owning Agent Attempt or independent Terminal binding and persists the close intent before runtime IO. Existing original-Core checks remain authoritative. Failed or uncertain termination keeps the tab reachable and displays an error. Closed entries/history remain available, and Agent conversations can explicitly Resume. Hiding the sidebar, leaving the page or quitting the UI only detaches the viewer and leaves runtime processes running.
- Manual Shells do not consume automatic Agent slots or inherit Agent idle reclamation. Running an Agent command manually in a Shell does not enroll it in Workflow or automatically move a Card.
- Manual Shell state is observed separately from Agent lifecycle, including original-Core checks. Archiving a Card retains its worktree if any independent Terminal is observed inside that worktree, regardless of its creation origin; unresolved live Terminal locations conservatively retain the checkout, and records the retention reason.
- Command+T creates, Command+W terminates the current Terminal and closes its tab, and Command+Shift+[ / ] cycles tabs while the Terminal workspace owns focus. Rename and Quick Open temporarily suspend Terminal shortcuts. Closing the active tab selects a neighboring tab; closing the last leaves a direct New terminal action.

## Implementation ownership

- `types::TerminalEntry` and storage: presentation identity, numeric allocation and schema migration.
- `app-server/terminals.rs`: manual Shell intent/receipt, rename, control, directory context and observation; no Scheduler admission.
- `ui/terminals/entries.rs`: common Agent/Shell projection and binding eligibility.
- `ui/terminals.rs`, `tree.rs`, `tabs.rs`, `viewer.rs`: navigation, split chrome, complete inventory, open tabs, secondary actions, rename and single selected native viewer.
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

The current title policy supersedes the numeric Agent labels used in earlier acceptance sections. Production `terminals/labels.rs` shares provider/Profile rendering between two-line inventory rows and single-line tabs; UI Kit owns the licensed provider SVGs. Presentation tests cover execution-stage ownership, auto/custom names, executable/preset marks and generic fallback. Storage tests cover v5-to-v6 backfill, independent numbering among Agent entries, immutable runtime bindings and clearing a custom name.

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
