# Card creation and editing

Status: implemented user-approved direction; native acceptance recorded 2026-11-10.

New Cards and existing task descriptions share the native Markdown source editor.
The default surface is the Board's right pane; Expand uses the main workspace,
and Collapse restores the pane without replacing the editor, selection, undo
history or scroll position. Clicking non-interactive Board space on the left
collapses an open right-pane Card or New Card back to the Board, and Escape
closes New Card when no completion menu or modal overlay is open. There is no
separate centered editing dialog.

Enter, Tab or a click accepts an open completion candidate; arrows select,
Shift+Enter inserts a newline without accepting, and Escape dismisses the list.
When completion is closed, Enter and Shift+Enter insert newlines.
Command+Enter dispatches the same guarded create/save intent as the button.
Source editors use an explicit `Menlo, PingFang SC` font cascade so per-grapheme
caret measurement matches CoreText line rendering across mixed Chinese and
English text.

Drafts are saved independently of published Card content. Closing or navigating
away retains a draft; only explicit confirmation updates a Card. A searchable Drafts screen
restores new Cards and pending edits after restart. Its sole entry is the counted
Drafts button in the Board header; the list includes all Projects with ownership
labels, keeps Board selected in the rail and provides an explicit return to Board.
Explicit discard removes the draft. Existing optimistic Card revisions prevent
overwriting concurrent edits.

At-sign completion references configured Profiles. Hash completion searches
registered `#tag` indexing tags, local Cards and, only while the GitHub account
is connected, the Project's configured GitHub repository (Issues and PRs),
with type, owner and number visible. When a typed `#name` query is a valid
non-numeric tag name (`1..=48` alphanumeric, `_` or `-` characters) that is not
yet registered, hash completion also offers **New tag · Create** (`#name`),
persisting the tag on the Project (`tag.create`) without blocking draft or
submission flow. Tags index Cards for Quick Open search and Board filtering;
they never route or launch Agents. References remain ordinary Markdown links or
`#tag` tokens; adding a reference or tag does not change the Card's source or
delivery identity or start an Agent. GitHub Issue Cards retain read-only source
content and editable local instructions. The comment composer retains explicit
recipient behavior.

UI Kit owns native editing, formatting, preview and keyboard policy. Product UI
owns candidates, Card forms and draft projection. AppServer owns durable draft
writes and GitHub queries. Draft persistence must not append Timeline events,
change workflow stages or create execution requests.

## Persistence and failure handling

Schema v9 adds `editor_drafts` without changing existing Card/history records.
`draft.list` returns draft values and versions; `draft.save` performs a versioned
replace or null tombstone. Tombstones prevent late saves from resurrecting
discarded text, and identical retries are idempotent. Desktop API level is 7.

One background writer coalesces checkpoints around 300 ms; ordinary application
exit requests a final flush. Successful autosave is silent (no "Saving draft…" flicker, 2026-11-13); only
failed writes are shown, with an explicit retry action. A crash before a checkpoint or a
failed write can leave recent input unpersisted. Concurrent-window conflicts are
reported rather than silently overwriting another saved draft. Card revisions
remain separate; a rejected Card save shows the latest saved description beside
the retained draft and allows explicitly adopting that revision before saving.

New Card drafts have independent identities in navigation history and submission
fences, retain the target Project and keep
Local description and GitHub instructions separately. Content-derived request
keys make retries of an identical create submission idempotent. Creating a Card
from New Card opens the newly created Card's details and replaces the submitted
form's history entry, so Back returns directly to the previous route. Any input
entered after submission remains available in Drafts. If the user navigated away
before acknowledgement, completion preserves their current route. Command+N opens
a new Card form when another Card is needed. Published
task readers do not present unsubmitted edits as saved content.

New Card submission completes when AppServer acknowledges the write, without
waiting for a full workspace refresh. The returned Card appears immediately;
one coalesced background refresh reconciles workflow and host state. Older or
cached snapshots retain acknowledged writes until authoritative state includes
them. A refresh failure is reported separately from a successful submission and
does not restore the submitted draft or invite a duplicate create. Host summaries
reuse the fetched snapshot rather than reading the full state again.

Text, recipients and references persist. Pending attachment bytes, native undo
history, caret and scroll positions are not restored across application restart.
Successful attachment links remain in the persisted Markdown. Expansion uses
the same mounted editor and preserves its selection/undo/scroll state. Narrow
windows use the available workspace directly.

GitHub reference search is debounced, cached per editor/source/query, bounded to
20 remote candidates, and fenced against account/source changes and stale native
completion requests. Local candidates do not wait for the network; failures keep
the editor usable and display an error. These are references, not Issue imports,
remote edits, delivery selection or Agent dispatch.

## Creation navigation acceptance (Card #103, 2026-11-17)

The packaged native app in `/private/tmp/cb-103-ocxy3v5f` used a dedicated home
and isolated Herdr Core. Create and Command+Enter each opened the acknowledged
Card's details with the saved title/body. Back returned directly to the Board,
and Command+N opened a blank New Card form. RPC reads confirmed one Card per
submission and tombstones for both submitted drafts. The scratch UI, AppServer
and Core were stopped afterwards.

All 572 workspace tests passed (one additional test skipped), including the
presenter/socket regression for opening details before snapshot refresh,
replacing the submitted route, and preserving a different draft's route when a
create response arrives late. Packaged build and isolated `mise run acceptance`
passed. Changed Rust files passed formatting checks; the workspace-wide format
check still reports existing differences in unrelated files. This run did not
exercise live GitHub imports or Chinese IME input.

## Readers, previews and navigation

Description, editor Preview, comments, replies and reports share the product
Markdown reader. Headings, emphasis, lists and code retain Markdown rendering.
Explicit links and recognized prose references have a hover preview and click
routing. Code and email-like text remain literal. Raw `#number` resolves locally only in a Project without a GitHub source. In a
GitHub-connected Project it remains literal until the user selects an explicit
Card/Issue/PR completion, avoiding ambiguous numbering and Issue/PR guessing.
GitHub-owned Cards rank remote candidates before local Cards once available.

- Local Card links show title, Project/number, stage and a short description;
  clicking opens the Card inside Canban, including archived Cards.
- Registered `#tag` tokens (and explicit `[...](tag:name)` links) render as
  clickable references in descriptions, previews, comments, replies and reports
  while ignoring code literals, blockquotes, URLs and unknown `#words` (such as
  hex colors). Hovering shows the matching Card count; clicking opens Quick Open
  pre-filled with `#tag` so matching Cards appear immediately.
- Known `@Profile` references show provider/role details and open local Settings.
  Reading or opening a reference never starts an Agent. Comment completion still
  adds an explicit recipient chip, independently of link navigation.
- Ordinary HTTP(S) links open the default browser. Web previews show the label,
  hostname and URL; they do not fetch arbitrary webpage content or screenshots.
- GitHub Issue/PR links show repository/type/number and available cached metadata.
  Connected root links open native GitHub details consistently with the reference
  summary. Fragment/query/subpath links retain their exact browser destination.
  Hover can load a short detail preview for a connected configured repository.
  Requests and the bounded 60-second cache are fenced by account/source identity.
  Disconnected GitHub does not perform remote completion or preview lookup;
  existing Markdown links remain readable and can still open in the browser.

Local completion filters the complete Card inventory before limiting the visible
results to 50. Native completion requests repeat as the query changes, refreshing
both candidates and the replacement range through the current caret. This avoids
omitting later matching Cards and leaving typed keywords behind after acceptance.

## Verification (2026-11-10)

- `mise run test`: all 173 workspace tests passed, including persistent draft restart,
  stale-save/tombstone protection, old-schema upgrades, UTF-16 Markdown trigger
  boundaries and HTTP fixture searches including PRs and exact numbers.
- `mise run build` and strict all-target Clippy for UI/UI Kit/AppServer/GitHub
  passed. `mise run acceptance` passed against its isolated home/Core, covering
  workflow, comments/mentions, archives, restart and global concurrency.
- `mise run gallery` with `CANBAN_GALLERY_SCENE=editor`: inspected the production
  form at 1440 and 760-point widths, expanded Markdown preview (heading, tasks,
  code), and Local/GitHub input separation.
- Packaged native application in `/private/tmp/cb-editor-s5azm3cp`: confirmed
  Enter/Shift+Enter newlines, Enter and Shift+Enter with an open @ menu, Tab
  insertion of a local Card Markdown link, selection replacement and undo across
  expansion/collapse, typing through a background snapshot, close/reopen retaining
  edits while showing published text, and restart recovery from Drafts.
- Native Command+Enter created each submitted Card once, saved an existing Card
  once, and sent an exact multiline comment without starting an Agent. Unicode
  paste/title and persisted Markdown were checked through AppServer reads. The
  final isolated binary matched the packaged binary's SHA-256. Rapid close/new
  reproduced a stale native title despite a blank second draft; keying the form
  by draft ID fixed it. The final build checked blank new input, Back/Forward
  restoring separate A/B drafts, and submitting B while retaining A. Exact snapshots,
  drafts and checks are in that directory's `native-evidence.json`.

Reproduce the disposable app with `mise run acceptance-editor`; close it normally
to stop only its own service/Core. Gallery fixtures and HTTP unit fixtures are not
live GitHub account acceptance. Live authenticated GitHub completion, Chinese IME
composition/candidate interaction and large-document latency have not been
accepted in this run. Unicode paste was accepted separately.

## Reference follow-up acceptance (2026-11-10)

Native isolated application evidence is retained in
`/private/tmp/cb-editor-7a84mxd7`. Before repair, typing `#Related` and accepting
left the keyword after the link, and `#needle` could not find Card #65. After
repair, the New Card editor and comment composer both found and inserted the
65th matching Card, and typed `@dev` inserted `@developer` without residue.
Exact persisted draft/comment bytes were checked through AppServer, with zero
Agent requests after removing the test recipient chip before sending.

Saved body and comment headings, bold/italic and code literals were inspected
in the packaged application. Local Card links in both readers displayed a
hover summary and navigated locally. Profile hover showed provider/role details;
click opened Settings. A comment website link displayed its URL preview; both body and comment
website links opened Chrome at their exact test URLs. A disconnected GitHub Issue link displayed
its repository/type/number without remote lookup. Preview remained selected
through expansion and collapse. The production Gallery `references` scene also
exercised Markdown, local hover summaries and Card navigation with 65 fixtures.

Workspace tests, packaged build, strict UI/UI Kit/AppServer/GitHub Clippy and the
isolated `mise run acceptance` suite passed. Authenticated GitHub remote search
and hover have HTTP/unit coverage but were not accepted with a live account.
