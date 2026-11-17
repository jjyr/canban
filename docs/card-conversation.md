# Card conversation workspace

Status: Card reading/editing redesign implemented 2026-11-10. See the dated verification sections for exercised builds and remaining limits.

## Layout and ownership (updated 2026-11-14: Card #58 / #66 unified Terminal toggle)

```text
+---------------------------------------------------+--------------------------------------------+
| Project / Card #42              [>_] [Diff] ... X | [◆ @developer · Working ×] [Terminal 1 ×]  |
| Card title                                        | [+]                   [Interrupt] [↗] [×]  |
| Workflow: Develop · branch                        +--------------------------------------------+
+---------------------------------------------------+                                            |
| Description (GitHub + local context)         Edit |  Live libghostty-vt Native Terminal        |
| References / Delivery PR                          |  (interactive discussion with @developer   |
| Activity                                          |   side-by-side while reading Card context) |
| ME [ Author · time / Markdown reply             ] |                                            |
| ✦  [ @developer · time / Markdown report        ] |                                            |
+-------------------- New activity (N) -------------+                                            |
| Contextual workflow approval/rework (when needed) |                                            |
| Write a comment… (@agent)                    Send |                                            |
+---------------------------------------------------+--------------------------------------------+
```

Card is a self-contained problem-solving workspace. Users can either post structured Timeline comments mentioning `@agent` on the left **or** work directly inside the **right-hand Live Terminal & Agent split** (`Toggle Terminal · ⌘J`, Board tile `[>_]` or `@profile` pill, `Open Card Terminal` in the context menu, clicking `#<number> <title>` / `Open Card` in `Terminals`, or `Toggle Card Live Terminal` in the Command Palette) side-by-side without leaving the Card view:

- **Collapsed by default on Card open:** Opening a Card directly (`open_card`, Quick Open `⌘O`, or ordinary Back/Forward history) keeps the right-hand Terminal split collapsed (`card_terminal_open = false`, without persisting prior expansion across closes or Card switches) while pre-selecting the Card's default live tab and tinting the header `[>_]` icon directly (`ACCENT`, or `ATTENTION` when waiting on user input, without a badge dot) when live sessions exist. Returning directly from Compare restores the prior split and selected live tab without starting a process. When all live entries on the Card finish or close, the right pane collapses automatically.
- **Unified `[>_ Terminal]` entry without separate Agent buttons:** Card header, Board card hover actions, Card context menus, and the Card Terminal pane header omit separate `Open Agent` / `New Agent` buttons and unify around `[>_]` / `+` (spawning a clean Shell in the Card's worktree with `CANBAN_CLI`, `CANBAN_CARD`, `CARD_ID`, `CANBAN_PROJECT`, `CANBAN_ENDPOINT`, and `CANBAN_RUN_DIR` injected, where users can run `claude`, `codex`, `agy`, or shell commands directly).
- **Left-right split geometry:** When the Card Terminal pane is closed, the Board remains visible beside the Card in wide layouts (or standalone Card when expanded/narrow). When the Card Terminal pane is open (`card_terminal_open`), the workspace presents the **Card reader + Timeline + composer on the left** alongside the **worktree-scoped multi-tab `canban/terminal` viewer on the right** via `canban_ui_kit::split` so both Card specification/comments and interactive Agent terminal output remain visible simultaneously. On narrow viewports (`< 950pt`), opening the Card Terminal switches cleanly within the Card workspace while keeping a one-click `← Card` return toggle in the header.
- **Single-attachment & non-destructive hide:** Only the `"card"` route mounts the Card's embedded `canban/terminal` viewer (never while `"terminals"` is active), guaranteeing a Herdr PTY stream is attached to at most one native viewer at a time. Hiding the Card Terminal pane (`×` / `Command+J`) detaches **only** the UI viewer without terminating any live PTY; clicking `×` on an individual tab terminates that PTY via `terminal.close`.
- **Explicit focus ownership:** Clicking inside the embedded Terminal pane grants native PTY keyboard focus (`card_terminal_focused = true`), whereas clicking the Card task editor or comment composer transfers focus cleanly to the Markdown editor (`card_terminal_focused = false`) so typing prose, `@` mentions, and `Command+Enter` never leak keystrokes into the PTY or get stolen by background terminal output. Global UI Command keys (`Command+O`, `Command+/`, `Command+I`, `Command+J`, `Command+N`, `Command+[` / `]`) and tab shortcuts (`Command+T`, `Command+W`, `Command+Shift+[` / `]`) remain intercepted by `ui_command_keys` when the embedded Terminal owns focus.
- **Worktree-scoped tabs & Pop out (`↗`):** The right-hand Terminal header displays horizontal tabs for all live workflow Agents, ephemeral CLI Agents, and Shells belonging to this Card, plus `+` (**New terminal**), `[Interrupt]`, `[↗]` (**Pop out** to the full-screen `Terminals` workspace), and `[×]` (**Hide Terminal pane**).

The Board keeps its ordinary columns and Cards, without execution-slot summary or filter buttons. The compact branch name is a direct **View Card changes in Git** action with `+N −M` totals from the default Card comparison. Its tooltip shows the full branch, target branch and file count. Right-click, the Card menu and the Command Palette expose **Copy Card branch** and **Refresh Card diff**. Totals include committed and uncommitted changes, including untracked files; loading is `…`, errors say **Diff unavailable**, and truncated comparisons are marked `≥`. An ordinary standalone Diff button is omitted when the branch action is present; the Review-stage action remains explicit. A Card without a workspace says **Workspace not created**; a PR-only Card exposes **View PR diff**. These states and their validation are documented in [Git workbench](git-workbench.md#direct-change-navigation). Host, path, base commit and manual workspace refresh are available under **... → Workspace details**, rather than consuming permanent conversation space. **...** owns stage moves, Archive/Restore, Delete/Remove, Stop workflow and context/attachment controls, all mirrored in the Command Palette (`>`). Board Cards expose the same lifecycle actions in their right-click menu. The X action is labeled **Close Card details** and only dismisses the pane. Review and human Continue remain directly reachable when applicable.

Opening/closing the pane uses a shared native reveal/fade while retaining the Board. Switching Cards does not replay the reveal. Composer resizing and shared dialogs use the same reduced-motion policy; see [native motion](ui-motion.md).

`card.rs` composes independently owned `card/header.rs`, `card/terminal.rs`, `task.rs`, `details.rs`, `timeline.rs` and `composer.rs`. These use shared UI Kit controls, NSH scroll/input/Markdown/menu behavior and existing product components. The standalone Gallery imports the same screen through the production frame.

## Description and composer

Description is fully visible in the common reader by default. **Edit** opens title/source editing with **Write / Preview**, formatting actions, **Save changes** and **Close editor**. Edit takes native source focus after its first mounted layout. Save uses the existing optimistic revision boundary; Closing retains unsubmitted changes in Drafts; the reader continues to show published content. New Cards and task editing share the right-pane/expanded source component; see [Card editor](card-editor.md). Command+Enter saves while this editor is active. GitHub-owned title restrictions continue to apply.

The comment composer uses the same UI Kit Markdown source editor and reader. NSH owns its rope, selection, caret completion and undo/redo history. Write/Preview and height changes keep the source editor mounted. Ordinary snapshot updates reconcile retained drafts and keyed Timeline rows without rebuilding the editing owner. Draft text, explicit recipients and saved resource references now survive application restart through AppServer draft storage. Pending attachment bytes remain window-local. Editor undo history is retained within the mounted editor, including expansion/collapse, and does not survive route destruction or application restart.

Focus expands the composer, with additional lines growing toward approximately one-third of the Card pane. Blur collapses only an empty composer; a nonempty draft stays expanded. Toolbar operations, caret completion and a native attachment picker belong to the editing interaction. Clicking the Card reading area ends composing focus; scrolling does not itself discard the draft. Preview prevents edits to the hidden source. Clipboard reads occur only in response to the explicit Paste shortcut, with a value/selection check that rejects a stale asynchronous insertion.

Enter inserts a newline. Command+Enter sends through the same intent as **Send**, with empty/pending/archived/attachment/modal checks. The UI Kit reclaims Command+Enter after each native editor focus event: NSH otherwise uses that key for a code-editor newline command. Attach suspends editor shortcuts, releases the real input focus before presenting the system sheet, and restores editing on selection or cancellation. Chinese IME acceptance remains excluded.

## Card identity and content references

The fixed header distinguishes local Card identity from GitHub Issue/PR identity.
GitHub repository/number and cached remote state are separate from the local
Workflow stage. Unknown remote state is labelled unavailable. The source opens
in Canban's native GitHub detail view through **GitHub discussion** (when connected)
and externally through **Open in GitHub ↗**. The cached source description remains
readable; **Edit local context** edits only the local body, never remote prose.

There is no permanent Add reference/URL form. The References row derives a
read-only index from the saved local body, cached source body and saved
comment/reply bodies, plus existing legacy explicit links. Explicit Markdown
links (including hash completion selections) and pasted GitHub URLs are indexed;
code literals and images are excluded. Drafts do not create relations. Removing
an occurrence from editable content removes that contribution; another saved
occurrence keeps the reference. Historical comments remain historical. Legacy
explicit links remain visible and retain the existing RPC removal path.

References deduplicate by repository/type/number or commit SHA. The Card's own
source and explicit delivery PR are excluded from ordinary references. Delivery
is never inferred from a pasted PR link. Neither referencing nor following a
link imports a Card, changes its source identity, publishes prose, or closes a
remote object. No new persisted relationship/schema is introduced.

## Conversation presentation

Card descriptions, comments, replies and reports support native text selection
and copying with Command+C. Selection applies to the rendered Markdown reader,
including expanded reports, while reference links retain their navigation behavior.

Native verification (Card #109, 2026-11-17): a packaged macOS app with an isolated
home/Core copied mixed English/Chinese description and reply text, selected across
paragraphs, lists and code, and copied six paragraphs from an expanded long message.
Clipboard contents were read back after copying. Copying a reply after typing in
the composer retained the unsent draft; a Card reference still navigated to its
target. The standalone Gallery's **Controls → Selectable Markdown** fixture also
passed selection/copy. Linux interaction was not exercised.

Human and Agent messages use the same identity marker, author/time header and
bordered Markdown body along a single continuous 28-point vertical spine. Long
bodies fold at a visual height while retaining the complete Markdown document;
**Read full report / Show less** toggles the body. The main reader is bounded
to 920 points in wide layouts. User-authored local comments (`comment.created` with `actor == "user"`) on
active Cards fold **Edit comment** and **Delete comment…** inside a compact `⋯`
popup menu on the comment header (also reachable via **Edit Latest Comment**
and **Delete Latest Comment…** in the Command Palette). Saving an edit via the
shared Markdown editor (`Command+Enter` / **Save** / **Cancel**) dispatches
`comment.update`, increments the comment's revision, shows a muted `· edited`
indicator at its original sequence position, appends a durable `comment.updated`
audit event, updates any still-pending Question text and derived GitHub URL
references, and preserves existing `@Agent` mentions without re-triggering them
(only newly added `@Agent` mentions enqueue new Questions). Confirming deletion
dispatches `comment.delete`, tombstones the original event in place (`kind ==
"comment.deleted"`, preserving per-Card sequence monotonicity while removing the
comment from Activity, derived references, and Agent prompt context), appends a
`comment.deleted` audit event, and cancels any still-pending Questions or
not-yet-started queued Agent runs triggered solely by that comment while leaving
running Agents and delivered replies intact.

Narrative messages (comments, explicit replies and un-echoed execution reports)
pair full 28-point avatars with bordered cards, while milestone updates (stage
transitions, approvals, PR publications, merges, unresolved input waits and
failures) render as compact semantic badge nodes centered on the same spine
axis. Consecutive milestone updates collapse to their latest entry with an
inline **`+N earlier` / `Hide N earlier`** disclosure pill (and an attention dot
when a folded step includes an alert), breaking cleanly at narrative messages.
Durable sequence order and report/reply identity deduplication are preserved.
Opening a targeted event expands its folded milestone group automatically.
Selected execution details expand inside the right column of that request's
earliest rendered row (or at the end if its events are absent) so the vertical
spine remains continuous.

Routine Workflow blocks no longer precede the discussion. Approval/rework
controls appear above the fixed composer only when required; Stop lives in the
Card menu. Workflow decisions remain distinct from Terminal permission/input.
The composer explicitly labels its local scope; this change adds no GitHub
comment publishing control.

## Recipients and attachments

Typing a standalone `@` opens configured Profile candidates at the caret; an explicit `@` (**Mention an Agent**) button in the composer toolbar also opens the candidate picker. Typing filters candidates; Up/Down select, Enter/Tab/click accepts and Escape dismisses. Shift+Enter inserts a newline while candidates are visible, and Enter/Shift+Enter insert newlines when candidates are closed. Completion and prose mention detection ignore email-like text (`user@profile`), quoted lines (`>`), escaped `\@`, inline backticks and fenced code blocks. Standalone `@profile` tokens anywhere in prose (leading or mid-sentence) surface removable recipient chips above the composer; clicking `×` on a chip removes a leading `@profile` prefix or suppresses a mid-sentence `@profile` from the explicit `mentions` array while keeping prose intact. The **"Local discussion · not posted to GitHub"** scope note is shown only when the Card is linked to a GitHub Issue or Pull Request.

Hash completion uses the same `#tag` (including **New tag · Create**), local Card
and connected GitHub Issue/PR source as the task editor. Candidates are recomputed
across the full inventory as the query changes. Description, comment, reply and
report readers share hover previews and navigation: `#tag` tokens open Quick Open
pre-filled with `#tag`, Cards open locally and Profiles open Settings. Connected GitHub
Issue/PR root URLs open the native GitHub detail; disconnected links and links
with query/fragment/subpaths open the browser, preserving their exact target.
Other HTTP(S) destinations open the browser. Code literals remain literal. See [reference behavior and
native acceptance](card-editor.md#readers-previews-and-navigation).

Desktop comments always send an explicit `mentions` array (including `[]`); legacy clients omitting the field keep the server's existing parser. An asynchronous send clears only the submitted draft, preserving newer text/recipients and failed submissions. No CLI interface change is required.

Send completes on the write acknowledgement and projects the returned comment
into Activity immediately. It does not wait for a full workspace/host refresh.
Refresh requests coalesce in the background; a refresh requested during an
in-flight read is retained, and older or cached snapshots cannot hide an
acknowledged comment while catching up. A failed refresh is a separate workspace
error, not a failed Send: the already submitted draft stays cleared. Actual write
failures retain the draft for retry, and pending submission guards still apply.

**Attach files** opens the native multiple-file chooser directly. Client-local paths are read off the render thread and sent as bytes, so they do not assume an AppServer on the same machine. Command+V supports ordinary text and clipboard images/files through the NSH clipboard service; file drops use the same upload boundary. Per-file chips expose uploading, success, removal and failure/retry. Pending/failed uploads block submission until finished, retried or removed. Uploaded bytes are shared in memory while pending rather than copied on every keystroke. Removing a chip removes its draft reference; the already saved attachment and its Timeline event remain Card history. Uploads are not a filesystem deletion operation.

Attachment references are readable links to Card-managed resources. Images referenced by normal Markdown image URLs use NSH's Markdown renderer; uploaded images currently appear as attachment links/chips rather than inline image thumbnails. Existing saved attachments remain reachable under **... → Attachments**. There is no path-entry panel in the attachment flow.

## Timeline and reading behavior

Description, events, comments, replies and reports share one vertical scroll owner. First open starts at the description. Activity connects conversation, stage moves, decisions, merges and unresolved problems along a vertical spine in sequence order, hiding the section heading completely when no visible activity exists yet. Process bookkeeping and low-value card metadata mutations are not shown (updated 2026-11-14, Card #42): `card.created`, `card.updated`, `card.column_entered/left`, `execution.queued/started/attached/continued/disconnected/reconnected/archived`, resolved `execution.input_required` prompts, idle-close progress, `review.merge_started` and `workflow.started/completed/stopped` stay durable for triggers and history but are filtered from the Card Activity projection; a deep link to any such event still reveals it, and consecutive milestone rows collapse into their latest update with an inline `+N earlier` toggle. Comment avatars use the signed-in GitHub account picture for the local user ("ME" while signed out or loading) and the colored provider icon (`canban_ui_kit::icon_colored`) for Agent messages. Narrative bodies share the UI Kit Markdown reader (headings, lists, quotes, code, tables, links and images). File-backed reports load once per keyed row, including archived resources. A report echoed as a reply is hidden when the request and report identity/resource path match; a report without a path remains a visible message. Late echoes update that row's presentation without duplicating the report body.

The Card saves its reading offset per route. Scrolling away stops following; new activity exposes an explicit jump-to-latest action. At the bottom, new replies and loaded report growth stay visible. Composer growth leaves a reader's offset intact. Draft/offset retention is in-memory for the window lifetime. Agent/Terminal operations and human approval boundaries remain separate from reading and commenting.

## Terminal boundary (updated 2026-11-14, Card #58 / #66)

Inside the Card workspace, opening a Card directly keeps the right-hand Live Terminal split collapsed (`card_terminal_open = false`) and indicates active sessions by tinting the `[>_]` header icon (`ACCENT`, or `ATTENTION` when waiting on user input, without a badge dot). Clicking a Board tile's `[>_]` icon (`Open Terminal for Card #N`), active workflow/CLI Agent pill, Timeline `Open Terminal`, or Card **Toggle Terminal (`⌘J`)** navigates to the Card workspace (`page = "card"`) and attaches (or directly spawns a Card Shell via `terminal.create`) inside that same right-hand split without intermediate landing screens or forced full-page jumps to `Terminals`. An exited Agent status in Overview or Timeline history offers **Resume** and explicitly restores its exact session via `agent.open` into the Card's right-hand split; passive selection alone never starts or resumes an Agent.

Within the Card's right-hand Terminal pane header, horizontal tabs switch between all live Agents and Shells for the Card, `+` opens a Shell in the Card's worktree, `[Interrupt]` interrupts the running workflow Agent, `[↗]` pops the active tab out into the full `Terminals` workspace, and `[×]` / `Command+J` hides the right split without killing any PTY. Closing an individual tab terminates its PTY via `terminal.close`.

## Earlier verification (2026-11-09)

These checks covered the earlier compact Card implementation. Its default disclosure and Activity grouping are superseded by the reading/editor contract above:

- Workspace Rust tests and strict all-target Clippy; AppServer regressions for explicit empty/single recipients, parser tests for quoted/inline names, duplicate prefixes and whitespace, and Timeline report-echo projection tests.
- `mise run acceptance`: isolated real Herdr connection, Develop → Review, mentions/attention, archive/report restoration, service restart and cross-Board global scheduling.
- Native `.local/Canban.app` build, copied into an isolated acceptance bundle/home/Core with deterministic provider commands: Enter retained a multiline comment; Command+Enter saved it exactly once; a prose `@claude` created no Claude request. Explicit @developer delivered one Question and an inline reply.
- Native existing-Agent navigation retained the draft on return. A Card with no history opened an empty Card-context Terminal view; explicit Choose Agent then started its fixture conversation. Task edit/save and on-demand workspace details worked.
- Gallery: full/narrow conversation layouts, long task/history, bottom-following, reading older messages while a new reply arrives, and the new-activity affordance. Use **Card conversation** and **Append reply** to reproduce.

This is deterministic integration/UI acceptance; it does not establish provider authentication readiness or a new real-model acceptance run. Chinese IME was excluded at the user's request.

## Direct lifecycle actions and recovery (updated 2026-11-11)

Local Cards offer **Archive Card** while active and **Restore Card** while archived. GitHub source Cards additionally offer **Remove from Board**, a reversible local sync exclusion through `card.remove`, retaining the Card number, content and Timeline. Removal cancels queued work and excludes the Card from normal snapshots and dispatch. Live or uncertain Agent invocations must be ended/resolved first; a failed removal leaves the Card active. No GitHub Issue or PR is closed or deleted.

[**Archived Cards**](archive.md) shows a global Project → archived Card conversation tree, hides empty Projects, and filters metadata with 120 ms debounced fuzzy search. Inline Restore and the Card header use the same lifecycle intent. Removed records are absent, including legacy records with an archive timestamp; Archive makes no separate removed-record fetch. The compatibility removal/restoration API and existing removed data remain intact. Archive retains and compresses completed work, and Restore does not schedule work. There is no additional Close workflow stage.

Historical isolated checks on 2026-11-09 covered header Archive → archived list → Restore, Board right-click Delete → Deleted Cards → Restore with the same Card number, and X dismissing only the detail pane. The Delete/recovery controls were superseded by Card #11; see the [2026-11-11 Archive acceptance](archive.md#verification-card-11-2026-11-11). Backend compatibility tests still cover queue cancellation/history preservation and rejection of removal with a live Agent. Gallery now labels its production scene **Archived Cards**, retaining hidden legacy removed fixtures as regression data.

[Terminal interaction and navigation](terminal-interaction.md) records the current delayed-exit/inactive contract and unified Back/Forward behavior.

## Redesign verification (2026-11-10)

- `mise run build`, `mise run test` (149 passing Rust tests), strict workspace/all-target Clippy, and `git diff --check` passed. Regressions cover UTF-16 mention context, prose/email/quote/code exclusions, sequence ordering and report/reply echo projection.
- `mise run acceptance` passed against an isolated Herdr 0.1.18 Core: Develop/Review, mentions/attention, archive/report restoration, service restart and cross-Board admission. This is deterministic runtime integration, not a new authenticated Codex/Claude run.
- The standalone production-component Gallery was checked at full and constrained widths: full description, Markdown headings/lists/quotes/tables/code, caret Profile candidates, formatting a selection, Preview/Write with undo retained, delayed reply while typing, and attachment failure/retry/removal. A failed attachment kept the draft and blocked Send; removing it enabled Send again.
- The packaged `.local/Canban.app` was copied into a task-owned acceptance bundle with its own Canban home, source repository and Core socket/state/config. Native checks covered expansion/collapse with draft retention, the common scrolling region, description Cancel and Command+Enter Save, exact multiline comment submission once, Card switching/Back with a retained draft, and typing/undo through a background `comment.add` refresh without focus loss.
- Selecting `@developer` inside a sentence created exactly one configured-profile request over a real Herdr PTY; the deterministic provider replied and completed. A subsequent ordinary `@claude` reference and email created no additional request. Snapshot assertions checked those counts and persisted bodies.
- The native chooser selected three disposable files together; removal omitted the selected draft reference from the submitted comment. Return accepted a selected file. Selection/cancellation restored editing, including Command+Enter after the chooser. Copying pixels from the disposable PNG in macOS Preview and pasting with Command+V created an `image/png` attachment and persisted its reference in a comment. Uploaded file/image bytes were checked against the managed files; the final smoke check matched the acceptance bundle binary to the packaged build and saved an exact description/comment. The last focus-entry adjustment was followed by targeted UI tests and all-target UI Clippy.

The original ordinary-Input focus loss was not reproduced with basic multiline typing during baseline inspection; its historical cause is not established. The redesign's mounted editor and update/selection/undo behavior were checked directly. A separate, confirmed integration defect was NSH Editor's default Command+Enter newline binding taking precedence over Send; the scoped post-focus submission binding fixes that behavior, including focus restored after a native sheet.

Limits: Chinese IME was excluded as requested. Native file drag/drop and Finder file-copy paste were not exercised in this run; they use the same client-byte upload path. The system chooser's Command+A did not select all during automation; multi-selection through Shift-click and acceptance with Return were verified, and the Command+A cause is not established. Uploaded images remain attachment links/chips, without inline attachment thumbnails. No live user's Canban home or Herdr Core was used.
