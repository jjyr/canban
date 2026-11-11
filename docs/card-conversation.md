# Card conversation workspace

Status: Card reading/editing redesign implemented 2026-11-10. See the dated verification sections for exercised builds and remaining limits.

## Layout and ownership

```text
+---------------------------------------------------------------+
| Project / Card #42 OR GitHub Issue/PR repository#42   GitHub ↗ |
| Card title                                  Terminal ...  X   | fixed
| Workflow: Review    branch                  Review / Continue |
+---------------------------------------------------------------+
| Description (GitHub source + distinct local context)     Edit | scroll
| References: Issue/PR links from saved content, when present     |
| Delivery PR: explicit delivery relation, when present          |
| Activity                                                      |
| JJ [ Author · time                                            |
|      Markdown reply                                         ] |
|    > developer · 2 execution events                            |
| AG [ Author · time                       Details / Terminal   |
|      Markdown report · Show full report                     ] |
| · Failure or human decision stays individually visible         |
+-------------------- New activity (N) --------------------------+
| Contextual workflow approval/rework, only when required        |
| Local discussion · not posted to GitHub                        |
| Write a comment…                                      Send    | fixed
+---------------------------------------------------------------+
```

The Board keeps its ordinary columns and Cards, without execution-slot summary or filter buttons. Wide layouts retain the Board beside the Card; narrow layouts use the Card pane with a Back action. Branch text is compact, copyable and has a full-value tooltip. Host, path, base commit and manual workspace refresh are available under **... → Workspace details**, rather than consuming permanent conversation space. **...** owns stage moves, Archive/Restore, Delete/Remove, Stop workflow and context/attachment controls. Board Cards expose the same lifecycle actions in their right-click menu. The X action is labeled **Close Card details** and only dismisses the pane. Review and human Continue remain directly reachable when applicable.

Opening/closing the pane now uses a shared native reveal/fade while retaining
the Board. Switching Cards does not replay the reveal. Composer resizing and
shared dialogs use the same reduced-motion policy; see [native motion](ui-motion.md).

`card.rs` composes independently owned `card/header.rs`, `task.rs`, `details.rs`, `timeline.rs` and `composer.rs`. These use shared UI Kit controls, NSH scroll/input/Markdown/menu behavior and existing product components. The standalone Gallery imports the same screen through the production frame.

## Description and composer

Description is fully visible in the common reader by default. **Edit** opens title/source editing with **Write / Preview**, formatting actions, **Save changes** and **Close editor**. Edit takes native source focus after its first mounted layout. Save uses the existing optimistic revision boundary; Closing retains unsubmitted changes in Drafts; the reader continues to show published content. New Cards and task editing share the right-pane/expanded source component; see [Card editor](card-editor.md). Command+Enter saves while this editor is active. GitHub-owned title restrictions continue to apply.

The comment composer uses the same UI Kit Markdown source editor and reader. NSH owns its rope, selection, caret completion and undo/redo history. Write/Preview and height changes keep the source editor mounted. Ordinary snapshot updates reconcile retained drafts and keyed Timeline rows without rebuilding the editing owner. Draft text, explicit recipients and saved resource references now survive application restart through AppServer draft storage. Pending attachment bytes remain window-local. Editor undo history is retained within the mounted editor, including expansion/collapse, and does not survive route destruction or application restart.

Focus expands the composer, with additional lines growing toward approximately one-third of the Card pane. Blur collapses only an empty composer; a nonempty draft stays expanded. Toolbar operations, caret completion and a native attachment picker belong to the editing interaction. Clicking the Card reading area ends composing focus; scrolling does not itself discard the draft. Preview prevents edits to the hidden source. Clipboard reads occur only in response to the explicit Paste shortcut, with a value/selection check that rejects a stale asynchronous insertion.

Enter inserts a newline. Command+Enter sends through the same intent as **Send**, with empty/pending/archived/attachment/modal checks. The UI Kit reclaims Command+Enter after each native editor focus event: NSH otherwise uses that key for a code-editor newline command. Attach suspends editor shortcuts, releases the real input focus before presenting the system sheet, and restores editing on selection or cancellation. Chinese IME acceptance remains excluded.

## Card identity and content references

The fixed header distinguishes local Card identity from GitHub Issue/PR identity.
GitHub repository/number and cached remote state are separate from the local
Workflow stage. Unknown remote state is labelled unavailable. The source opens
externally through **Open in GitHub**. The cached source description remains
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

Human and Agent messages use the same identity marker, author/time header and
bordered Markdown body. Long bodies fold at a visual height while retaining the
complete Markdown document; **Read full report / Show less** toggles the body.
The main reader is bounded to 920 points in wide layouts.

Only adjacent routine queued/started/attached/report-echo facts for the same
request collapse into an execution group. Comments, failures, questions, review
decisions and unknown event kinds break groups and remain visible. Durable
sequence order and report/reply identity deduplication are preserved. Opening
a targeted event expands its group. Selected execution details appear beside
that request's earliest event (or at the end if its events are absent), retaining
exact historical request identity and explicit Terminal navigation.

Routine Workflow blocks no longer precede the discussion. Approval/rework
controls appear above the fixed composer only when required; Stop lives in the
Card menu. Workflow decisions remain distinct from Terminal permission/input.
The composer explicitly labels its local scope; this change adds no GitHub
comment publishing control.

## Recipients and attachments

Typing a standalone `@` opens configured Profile candidates at the caret. Typing filters them; Up/Down select, Tab/click accepts and Escape dismisses. Enter and Shift+Enter insert newlines even while candidates are visible. Completion is suppressed in email-like text, quoted lines and code. Selecting a candidate adds an explicit, removable recipient chip, including when the mention occurs within a question. Editing/undoing source text does not silently remove an explicitly selected recipient; the chip controls that address. Recognized consecutive leading `@profile` tokens retain their existing addressing behavior. Ordinary prose/email/code references do not become recipients merely by appearing in text.

Hash completion uses the same local Card and connected GitHub Issue/PR source as
the task editor. Candidates are recomputed across the full inventory as the query
changes. Description, comment, reply and report readers share hover previews and
navigation: Cards open locally and Profiles open Settings. Connected GitHub
Issue/PR root URLs open the native GitHub detail; disconnected links and links
with query/fragment/subpaths open the browser, preserving their exact target.
Other HTTP(S) destinations open the browser. Code literals remain literal. See [reference behavior and
native acceptance](card-editor.md#readers-previews-and-navigation).

Desktop comments always send an explicit `mentions` array (including `[]`); legacy clients omitting the field keep the server's existing parser. An asynchronous send clears only the submitted draft, preserving newer text/recipients and failed submissions. No CLI interface change is required.

**Attach files** opens the native multiple-file chooser directly. Client-local paths are read off the render thread and sent as bytes, so they do not assume an AppServer on the same machine. Command+V supports ordinary text and clipboard images/files through the NSH clipboard service; file drops use the same upload boundary. Per-file chips expose uploading, success, removal and failure/retry. Pending/failed uploads block submission until finished, retried or removed. Uploaded bytes are shared in memory while pending rather than copied on every keystroke. Removing a chip removes its draft reference; the already saved attachment and its Timeline event remain Card history. Uploads are not a filesystem deletion operation.

Attachment references are readable links to Card-managed resources. Images referenced by normal Markdown image URLs use NSH's Markdown renderer; uploaded images currently appear as attachment links/chips rather than inline image thumbnails. Existing saved attachments remain reachable under **... → Attachments**. There is no path-entry panel in the attachment flow.

## Timeline and reading behavior

Description, events, comments, replies and reports share one vertical scroll owner. First open starts at the description. Routine events are compact rows in sequence order, without a separate Activity disclosure. Narrative bodies share the UI Kit Markdown reader (headings, lists, quotes, code, tables, links and images). File-backed reports load once per keyed row, including archived resources. A report echoed as a reply remains a compact history event when the request and non-null resource path match; a report without a path remains a visible message. Late echoes update that row's presentation without duplicating the report body.

The Card saves its reading offset per route. Scrolling away stops following; new activity exposes an explicit jump-to-latest action. At the bottom, new replies and loaded report growth stay visible. Composer growth leaves a reader's offset intact. Draft/offset retention is in-memory for the window lifetime. Agent/Terminal operations and human approval boundaries remain separate from reading and commenting.

## Terminal boundary

Clicking an existing live Agent status or Timeline Agent name navigates to its conversation. An exited Agent status is labeled Resume and explicitly restores its exact session; it does not open an ended-session page. Live navigation does not resume or start an Agent. **Open Card Terminals** selects an existing Card conversation if available; otherwise it opens a Card-context empty Terminal view without selecting an unrelated tab or starting a process.

The Terminal chrome offers **Choose Card Agent** for an active associated Card. The empty Card view offers **Choose Agent**. This explicit action invokes the existing `agent.open` Start/Resume boundary. Ended sessions retain Resume/Start fresh; interrupt, close and history remain Terminal controls. **Back to source** returns to the Card and its draft. Closing a Terminal continues to use the existing unified lifecycle close operation.

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
