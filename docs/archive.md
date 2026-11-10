# Archive browser

Status: implemented, 2026-11-10.

Archive is a global browser of archived Card conversations, independent of the selected Board Project. Canban conversations belong to Cards; Agents remain metadata inside those conversations rather than a third navigation level.

```text
Archive
[ Search projects, Cards, numbers or Agents…       × ]

v  Project A                                      2
   |  #42 Improve search                  Restore
   |  #39 Preserve session history        Restore
v  Project B                                      1
   |  #7 Fix reconnect                    Restore

>  Deleted Cards                                  1
   v  Project C                                   1
      |  #3 Recover deleted work          Restore
```

## Hierarchy and interaction

Project headers use a folder, disclosure arrow, item count and indented guide. Groups start expanded; clicking a header or pressing Space/Enter while it is focused toggles it with the shared disclosure animation. Projects without archived Cards are absent. Deleted-only Projects appear exclusively inside the separate, initially collapsed Deleted Cards recovery group.

Rows show Card number/title, archive age and known Agent profile names. Long titles truncate. Clicking an archived row opens its existing Card history; it does not restore the Card, resume an Agent or schedule work. Back returns to Archive with the search and normal group expansion state retained for the current App session. Restore uses the same lifecycle intent as the Card header. Restoring a Project's last archived Card removes that Project from this tree. Deleted records have Restore without an unusable history-open control.

Projects sort by name and Cards by newest archive time, with stable tie-breaking. Matching relevance takes precedence while searching. One scroll owner contains the tree; the search field stays above it.

## Search

The native controlled field settles after **120 ms without another edit**, then filters locally without requiring Enter. Each edit cancels the previous timeout. The field is outside the filtered result subtree, preserving focus, selection and continuous typing.

Search reuses Quick Open's case-insensitive Unicode subsequence matcher. Whitespace-separated terms must all match, potentially across different fields: Card title, number, Project name, Agent profile or provider/command name. Exact/prefix/contiguous title matches rank ahead of loose subsequences and secondary metadata. Search does not scan full conversation bodies, reports or compressed files.

Only matching Cards and their nonempty parent Projects remain. A matching Project name naturally includes its archived Cards. Search expands matching groups; users may still collapse them. Clearing the query restores the normal expansion state. A no-match message replaces the tree when appropriate. Deleted recovery uses the same filter and expands when it contains matching records.

## Ownership and data access

- `ui/archive/model.rs`: pure metadata projection, grouping, matching and ordering.
- `ui/archive.rs`: screen composition, transient search state and generation-fenced deleted-record loading.
- `ui-kit/search.rs`, `input::debounced`, `tree_group`: reusable matching, debounce and native disclosure contracts; no product or storage dependencies.

Archived Cards come from the App snapshot. Deleted recovery uses the existing global `card.list(include_removed: true)` operation on page entry and Card lifecycle changes, with a visible retry on failure. Typing does not query AppServer, Herdr, Git or the filesystem. Current snapshot records override stale cached deleted records, so a restored Card cannot reappear from an older recovery response. No database/API migration is needed.

## Verification (2026-11-10)

- `mise run test`: 162 workspace tests passed, including Archive grouping, empty/active/deleted separation, multi-term/Unicode/number/Agent search, stale recovery prevention and existing Quick Open regressions.
- `mise run build`, `mise run build-gallery` and strict workspace/all-target Clippy passed.
- `mise run acceptance`: isolated real-Herdr deterministic workflow, compressed archive/read/restore, restart and cross-Board admission passed. This does not invoke authenticated Codex/Claude models.
- Native production-component Gallery: rapid query replacement, number/profile matching, no-results, focus retention, separate deleted recovery, search-clear restoring prior folds and the final tree spacing at the 720-point Gallery viewport.
- Packaged native App with scratch data/Core at `/private/tmp/cb-archive-nbhnqy88`: selected an active-only Project, opened the global tree, searched across Project/Card fields without Enter, opened archived history and returned with the query retained, restored the last archive in Tools, and restored a deleted Card. Empty archive Projects and the emptied recovery group disappeared immediately. Space/Enter toggled a Project; pasted Chinese subsequences filtered correctly. CLI snapshots confirmed persistence and zero Agent requests/attempts after inspection and restoration.

Chinese IME and a new real-provider Agent run are outside this Archive acceptance scope. Search timing is configured at 120 ms; native checks verify automatic updates and rapid replacement, not an instrumented end-to-end latency measurement.
