Update 2026-11-08: the Terminal portion of the focused-work layout below has been replaced by the [dedicated Terminal workspace](terminal-workspace.md). Card/Overview/search/Note execution links open its stable conversation tree and native viewer. Review remains in its existing focused-work composition. Earlier acceptance is dated evidence for the former layout.

# Notes and workspace implementation

Status: implemented and checked on macOS, 2026-11-07. This records the approved Kanban-centered composition and database Note model. It does not turn the older UI audit into current runtime evidence.

## Ownership and data path

```text
Native NSH Notes editor                 Agent in Herdr terminal
         |                                      |
         | typed AppServer call                 | canban note ...
         |                                      | Unix socket JSON RPC
         +------------------+-------------------+
                            |
                         AppServer
                            |
                    one SQLite transaction
                      + current Note
                      + immutable revision
                      + parsed outgoing links
                      + ordered Note events
                      + Card Timeline, if Agent-bound
                            |
                         UI refresh

Selected Note revision -> execution capture files -> immutable launch context
Live Note edits ----------------------------------> future captures only
```

A Note is a Project-owned database document, with a file-like name and read/write interface. No physical Note path, watcher or filesystem import migration is involved. `~/.canban/canban.sqlite` remains WAL-backed. Schema v3 adds `note_revisions` and `note_events`; an existing Note seeds its current saved revision without fabricated history. Revision bodies load on request rather than being duplicated into every application snapshot.

Current Note fields include stable ID, Project ID, name/title, Markdown body, revision, creation/update timestamps, parsed outgoing Note IDs and associated Card IDs. A rename preserves identity. Names must be nonempty and unique case-insensitively within the Project. Empty bodies are allowed.

## Operations and CLI

| Operation | Contract |
| --- | --- |
| `note.list` | Optional `project_id`; returns current documents |
| `note.get` / `note.read` | `note_id`, optional Project scope; returns current document |
| `note.save` | Create using Project/title/body, or update using Note ID and expected revision |
| `note.write` | Note ID, expected `revision`, required string `body`; preserves the name |
| `note.rename` | Note ID, expected `revision`, required string `title`; preserves content and ID |
| `note.history` | Note ID; ordered related events, including links and Card associations |
| `note.revision` | Note ID and revision; immutable title/body from that save |
| `card.create` | Optional `source_note_id`; selects this Note as context and records the Backlog Card relation |
| `card.context.update` | Existing Card/revision and `note_ids`; updates Note/Card relations consistently |

All mutation methods use the existing AppServer idempotency boundary and accept `request_key`. Stale revisions reject the complete write, including history/link changes. Identical writes do not create another revision. Save history and association-only events are different: a Card relation can append an event without rewriting the document body.

```sh
canban note list --project-id PROJECT_ID
canban note read NOTE_ID                 # exact body, no JSON/newline decoration
canban note read NOTE_ID --json          # includes ID and revision
canban note create --project-id PROJECT_ID --title Design.md --file draft.md
canban note write NOTE_ID --revision 3 --file updated.md
canban note rename NOTE_ID Architecture.md --revision 4
canban note history NOTE_ID
canban note revision NOTE_ID 3
```

Create/write reads standard input if `--file` is omitted or is `-`. A normal path is an input/output convenience, not canonical Note storage. Agents must read the current revision before writing and re-read after a conflict.

For Note mutations, the CLI injects `execution.request_id` and `execution.attempt_number` from the execution environment. AppServer validates that the request belongs to the same Project and the attempt is current. The event records `@profile`, Card, request and attempt; a matching Card Timeline row links back to the Note. Generic `canban call` Note mutations use the same injection. Unbound edits record the user actor. Reads do not append events. This is local provenance, not a new authentication boundary.

## Links, history and context

Use `[Architecture](note:NOTE_ID)` or a reference-style Markdown link. The Markdown AST parser ignores code examples, normalizes reference definitions and deduplicates outgoing IDs. Known cross-Project targets are rejected; missing targets remain visible as unavailable references. Backlinks derive from current outgoing IDs, so no reciprocal body mutation is necessary. Renaming a Note leaves existing links valid, although the author's written link label remains unchanged.

The Notes workspace provides link insertion, outgoing/backlink navigation, related Cards, event revision inspection and navigation to the related execution when available. Create Card opens an editable scope preview and saves into Backlog; it does not start an Agent. Cards created directly retain the previous default of all Project Notes; Note-created Cards explicitly select their source.

Execution snapshots contain the content/revision captured at launch. Later Note edits do not rewrite prompts or captured manifests. Card archives retain related Note events alongside the Card records; shared Notes remain Project-owned documents.

## UI composition and state

```text
Board browsing
  Sidebar | Board [scroll/stages] | selected Card [read/edit/timeline/composer]

Click Agent or Review changes
  Sidebar | Card [context/timeline/composer] | Terminal or Review
            <--- Board hidden during focused work --->

Expand work
  Sidebar | Terminal or Review
            Show Card / Show work / Restore split

Back / Close panel
  Sidebar | original Board position | selected Card and retained drafts
```

Board is the default destination. Card opens in reading mode, with explicit editing and secondary context/resources/stage actions. Shared status tones separate workflow, execution and attention. The composer remains outside the scrolling history region. Navigation uses selected states independently of hover/focus. Quiet controls, adaptive surfaces and common typography come from UI Kit.

Below 900 points of content width, Board/Card becomes one main surface; focused work toggles Card/work. Notes retain the list and switch the editor/related region. Wide layouts use NSH resizable panels. Split sizes, document drafts and Board column scroll live in root-owned memory across route disposal. They are not crash-persistent drafts. Errors remain near the affected editor; remote content changes retain local text and reject outdated saves until the user explicitly resolves them.

Sidebar/new-Note navigation invalidates pending output loads. Terminal selection is memoized by run identity so unrelated Note/snapshot updates do not remount its native view. Closing a panel changes navigation only; interrupt/end remain explicit Agent actions.

The Gallery lives in `examples/gallery` and imports production components. It supplies Board/Card, focused Review, Notes/link/history and shared-control scenes, with full/narrow viewport and light/dark switches. It opens no AppServer, database or Herdr connection. Use `mise run gallery`; the production app has no Gallery route.

## Acceptance evidence

Final checks on this working tree:

- `mise run test`: 42 Rust tests passed, including revision/link/provenance atomicity, conflict rollback, Project scope, Note-to-Card associations, schema upgrade and draft reconciliation.
- `mise exec -- cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `mise run build` and `mise run build-gallery`: passed.
- Formatting and `git diff --check`: passed.
- `mise run acceptance-interactive`: `INTERACTIVE_ACCEPTANCE_PASSED`, disposable home `/private/tmp/cb-interactive-u2uecv4q/home`, provider fixture ID `4b5c7b1a-eeac-4c85-84c4-3803ea187f01`. This uses real Herdr terminals with a deterministic Agent fixture, not a live-model claim. It covers actual Note CLI write/read/conflict/rename/provenance, immutable context and the existing interrupt/restart/outage/resume/archive paths.

Native application checks used `/private/tmp/cb-native-z36mvq7a`, its own Canban home and Core socket. They exercised:

1. Board/Card composition; scroll a column, enter Terminal, type input, close back and retain Board position/comment draft. Viewer close left the Agent available.
2. Note create/edit/save/preview, insert a stable link, navigate its backlink and rename without breaking references.
3. Keep an unsaved human draft, update the Note through the Agent terminal/CLI, reject the stale UI save, inspect the saved revision and explicitly replace the draft.
4. Create Card #14 with an edited scope from a Note; persisted Backlog state, source context and no execution request were checked.
5. Send an active `@developer` question; it replied in the original invocation. Finish Develop, observe automatic Review and separate Reviewer completion, then resume Developer with the exact original provider ID and complete again without another stage callback.

The final packaged build was then restarted once in the same disposable home. Persisted Note content, links, revision history and Agent provenance remained visible; opening a saved revision then choosing New cleared the old output. This one restart does not establish repeated-restart window-discovery reliability.

The saved snapshot/evidence (`workspace-snapshot.json` and `workspace-evidence.json`) under that scratch directory verifies Note revision 3 and Agent/Card/request/attempt provenance. Developer provider ID stayed `d9324361-f9aa-411e-a723-158d7bdf33d3` across initial execution and resume; all three Developer/Reviewer attempts completed and closed. The source Note retained the original and renamed revision bodies.

Final native Gallery checks used `/private/tmp/cb-final-gallery-0pbwbl1l`. Pointer resizing changed Board width from 700 to 762. Narrow Card/Review toggling retained a comment draft; narrow Notes switched between editor and relations with a return action. Light and dark appearances were inspected using the same production components. This constrained viewport check is not an exhaustive OS-window resize matrix.

## Remaining limits

Chinese IME checking was skipped at the user's request. Claude live authentication remains a separate blocker from earlier acceptance. Repeated native restart/window-discovery automation is not resolved by this update. Full drag/drop previews, autoscroll, design-state coverage, rich review UI and crash-persistent drafts remain refinements. The Note UI uses native Markdown editing/preview rather than a rich-text editor. No Note chat or another Session class was added.
