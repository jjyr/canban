# Archive browser

Status: implemented; Project-scoped Archive and automatic post-merge archive updated 2026-11-11 (superseding the earlier global rail Archive entry).

Archive is opened from the current Project's Board header (**Archive**, or inside **More…** on narrow viewports) and displays that Project's archived Card conversations. The left navigation rail no longer has a global Archive icon; **Board** remains selected while viewing a Project's Archive.

```text
← Board   Archive · Project A
[ Search archived Cards, numbers or Agents…        × ]

#42 Improve search   [Merged]                Restore
#39 Preserve session history                 Restore
```

## Hierarchy and interaction

Rows show Card number/title, a **Merged** status badge when delivered, archive age and known Agent profile names. Removed Cards are absent, including legacy removed records that also carry an archive timestamp. Clicking **← Board** returns to the Project's Board with retained column scroll.

Clicking an archived row opens its existing Card history; it does not restore the Card, resume an Agent or schedule work. Back returns to Archive with the search query and scroll offset retained for the current App session. Restore uses the same lifecycle intent as the Card header.

## Search

The native controlled field settles after **120 ms without another edit**, then filters locally without requiring Enter. Each edit cancels the previous timeout. The field is outside the filtered result subtree, preserving focus, selection and continuous typing.

Search reuses Quick Open's case-insensitive Unicode subsequence matcher. Whitespace-separated terms must all match, potentially across different fields: Card title, number, Project name, Agent profile or provider/command name. Exact/prefix/contiguous title matches rank ahead of loose subsequences and secondary metadata. Search does not scan full conversation bodies, reports or compressed files.

Only matching Cards and their nonempty parent Projects remain. A matching Project name naturally includes its archived Cards. Search expands matching groups; users may still collapse them. Clearing the query restores the normal expansion state. A no-match message replaces the tree when appropriate.

## Ownership and data access

- `ui/archive/model.rs`: pure metadata projection, grouping, matching and ordering.
- `ui/archive.rs`: screen composition and transient search state.
- `ui-kit/search.rs`, `input::debounced`, `tree_group`: reusable matching, debounce and native disclosure contracts; no product or storage dependencies.

Archived Cards come from the App snapshot. Archive performs no separate removed-record fetch. Typing does not query AppServer, Herdr, Git or the filesystem. No database/API migration is needed.

## Card lifecycle actions

Local Card menus offer **Archive Card** while active and **Restore Card** while archived. There is no Delete Card action or Deleted Cards recovery group. Archive retains history; it does not mean deletion. Archiving from the Board (including its Card detail pane) keeps the current Board and Project visible and closes the detail pane; the archived Card leaves the active columns. Open Archive explicitly from the Board header to browse archived Cards.

GitHub source Cards retain **Remove from Board** as a separate local sync-exclusion operation (see [GitHub Cards](product/github-cards.md#local-removal-and-restoration)). Existing removed data and the compatibility removal/restoration API remain intact; this UI change neither purges nor automatically restores records.

## Verification (Card #11, 2026-11-11)

- `mise run build`, `mise run test` (231 tests), `mise run acceptance`, formatting and `git diff --check` passed.
- Native production-component Gallery verified that legacy removed records (including those with an archive timestamp) and their Project are absent; searching for them produces no matches. Restoring Tools' last archived Card removes its group. GitHub Cards retain Remove from Board alongside Archive Card.
- The freshly packaged App, copied with a unique native identity and isolated home/Core under `/private/tmp/cb-card11-x0r69prf`, verified Board/detail menus without Delete Card, header Archive, archived history inspection, removed-record search exclusion and header Restore. Final CLI snapshots confirm the restored Card is active, the legacy removed record remains stored, and no requests or attempts were created.

These native checks use disposable fixtures, not authenticated model runs. The compatibility removal API was used only to prepare the legacy-record fixture.

## Historical verification (2026-11-10)

The deleted-record recovery checks below describe the earlier UI, superseded by Card #11.

- `mise run test`: 162 workspace tests passed, including Archive grouping, empty/active/deleted separation, multi-term/Unicode/number/Agent search, stale recovery prevention and existing Quick Open regressions.
- `mise run build`, `mise run build-gallery` and strict workspace/all-target Clippy passed.
- `mise run acceptance`: isolated real-Herdr deterministic workflow, compressed archive/read/restore, restart and cross-Board admission passed. This does not invoke authenticated Codex/Claude models.
- Native production-component Gallery: rapid query replacement, number/profile matching, no-results, focus retention, separate deleted recovery, search-clear restoring prior folds and the final tree spacing at the 720-point Gallery viewport.
- Packaged native App with scratch data/Core at `/private/tmp/cb-archive-nbhnqy88`: selected an active-only Project, opened the global tree, searched across Project/Card fields without Enter, opened archived history and returned with the query retained, restored the last archive in Tools, and restored a deleted Card. Empty archive Projects and the emptied recovery group disappeared immediately. Space/Enter toggled a Project; pasted Chinese subsequences filtered correctly. CLI snapshots confirmed persistence and zero Agent requests/attempts after inspection and restoration.

Chinese IME and a new real-provider Agent run are outside this Archive acceptance scope. Search timing is configured at 120 ms; native checks verify automatic updates and rapid replacement, not an instrumented end-to-end latency measurement.
