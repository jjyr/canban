# GitHub workspace and disposable cache

Status: integrated with the editor/updater on main, 2026-11-10. This is the current contract for GitHub navigation, source Cards, browsing and caching. It supersedes the earlier Issue-only intake, Settings entry and permanent remote-comment Timeline imports. Dated live acceptance in [GitHub integration](github-integration.md) remains historical evidence, not acceptance of the new UI.

## Navigation and account access

The primary rail places GitHub immediately above Archive. GitHub contains Notifications, Issues and Pull requests. Account/access and cache controls belong to its Account & access page. Settings no longer duplicates GitHub configuration.

The Board header includes a separate icon-only synchronization action next to the GitHub icon. Clock, rotating refresh (respecting reduced motion), warning and pause icons expose queued, running, failed/partial and paused states; tooltips show the repository, last successful synchronization, additions and errors. The action queues a manual synchronization without opening settings.

The Board header's GitHub icon opens the selected Project's repository and sync rules, showing a connected indicator dot when an active repository source is bound. Canban automatically inspects a Project's Git remote (`workflow.workspace.remote`, defaulting to `origin`, or a unique `github.com` remote) on Project creation, work-location updates, startup/background reconciliation, and post-sign-in checks. When a signed-in GitHub App or `gh` CLI account can resolve the repository, Canban automatically binds `Project.github` with both Open Issue intake (`intake`) and Open PR intake (`pr_intake`) defaulting to `off` — enabling Issue/PR search, GitHub panel scoping, and PR delivery immediately without flooding Backlog. Manually disconnecting a repository records `github_disconnected` so background reconciliation will not re-link it automatically until the user explicitly reconnects (including one-click linking of the detected remote). Existing linked projects preserve their configured intake modes, and ordinary local work and execution locations remain independent of GitHub availability.

Canban App authentication remains available through the existing Device Flow and private credential file. Repository operations prefer that account when connected. With no App connection, the already signed-in GitHub CLI account supplies repository browsing, intake and explicit API actions. Canban invokes `gh api` without extracting or copying its credentials. Git push continues to use the saved execution machine's Git credentials.

Notifications use GitHub CLI authorization. App and CLI account names must match when both are in use. The page identifies App repository access versus CLI access; neither promises access beyond the selected authorization. App permissions still govern its actions: Issue comment publication requires Issues write permission, which the originally registered App did not request. CLI fallback has the permissions of its own authorization. A failed API action never silently switches accounts.

## Card ownership and references

Cards have Local, GitHub Issue or GitHub PR content. Each source Card has one stable repository/object identity, separate from its local Card number. A Project has at most one source Card per remote object. Source title/body/state are cache-backed projections; editable instructions, stage, execution history and human decisions remain local.

All Card kinds support explicit Issue, PR and commit URL references. Pasting and adding a reference keeps a Local Card local. References open the common GitHub detail workspace. Source, related reference and delivery PR are different relationships. An existing delivery PR opens its owning Card and is excluded from duplicate automatic PR task creation.

The unified Add Card form supports Local, GitHub Issue and GitHub PR. Search/select/preview precede Add; manual addition includes closed sources and is independent of intake eligibility. Creation and restoration use the shared numbering/lifecycle transaction. Removed sources stay excluded until explicitly restored. Active or uncertain execution still prevents removal.

## Intake and Board filters

Issue intake supports off, all Open Issues and collaborator-authored Open Issues. PR intake supports off, all Open PRs, collaborators, assigned to me, authored by me or Review requested from me. Additional author and Label lists narrow new intake, and Draft inclusion is explicit. Author matching is case-insensitive; collaborator membership uses GitHub user IDs. Label alternatives are OR; distinct condition groups are AND.

Automatic intake creates Backlog records without Agent, Session or worktree launch. Changed eligibility never ejects existing Cards. Turning intake off continues to refresh already tracked sources. Pause stops automatic synchronization; explicit manual synchronization still runs once; independent GitHub browsing can still refresh the shared remote cache. Closing an Issue or merging an ordinary related PR does not move or archive a Card. A delivery merge records a local delivery receipt through the existing workflow policy; Archive remains separate.

Accepted next architecture (2026-11-11, initial implementation; see [workflow/event coverage](product/workflow-engine.md)): [event subscriptions](product/events-and-notifications.md) can react to source-bound Card creation and Issue/PR observations, and start the [unified workflow engine](product/workflow-engine.md). Intake itself retains the Backlog-only ownership action; execution requires an enabled subscription. Source bindings must be complete before publishing creation. Discovery/baselines, polling gaps and durable deduplication are explicit. Persist minimal event/progress metadata independently of disposable responses; do not restore permanent remote-body/comment/diff mirrors. Existing refresh broadcasts are not this durable event feed. A configured workflow may explicitly react to source close/merge; these observations still have no built-in Card progression.

The Board filter bar combines source/type into Type and provides Author (including Mine), Tags and More (GitHub state and Scheduled). Filters persist per desktop account and host-qualified Project across restarts and affect visibility only. See [Unified Board filters and persistence](#unified-board-filters-and-persistence) for identity, reset and unavailable-source behavior.

## Remote lists and PR workspace

Lists use remote filtering and pagination. Notifications request 50 items per page; Issue/PR search and detail collections request 30. Previous/Next comes from the API Link header, with no fabricated page count. Browsing does not create Cards. Existing source/delivery associations open the matching Card, including archived Cards; otherwise Create sync Card selects a connected Project, or directs the user to repository configuration. Multiple matching Projects require an explicit target.

Remote query pages are cached independently by authorization, endpoint, complete query and page. Cached summaries are not a complete local index and are never assembled into a pretend remote page. List scroll positions and query/page state survive navigation away and back within the window.

Changing a list filter resets it to page 1. A successful explicit first-page refresh invalidates its cached sibling pages because membership and ordering may have changed. Per-query epochs prevent older in-flight later-page reads or refreshes from restoring those invalidated entries; unrelated queries remain cached.

The common detail view offers Overview and Discussion; PRs add Files changed, Reviews, Review threads and Checks. Missing/binary patches have an explicit GitHub fallback. Viewing a PR does not create or change a checkout. GitHub PR diff and the existing local workspace Review diff remain separately labelled views.

PR file pages are cached under both comparison SHAs. On a miss or explicit refresh, Canban checks the PR before and after fetching the files. A moving comparison is rejected rather than cached under the wrong version. Line comments choose the file, side and line number and collect into a pending Review. A Review contains the displayed head SHA, and submission revalidates it against GitHub. New commits do not silently re-anchor a draft.

Comment and Review forms expand on demand so the diff retains its reading area. Every remote comment across Discussion, Reviews, Review threads, and PR Diff & Review inline threads exposes an external **GitHub ↗** button on its header (`html_url` / section anchor fallback; also reachable via **Open Latest GitHub Comment in Browser** in the Command Palette) to open that exact comment anchor in the system browser, while folding own-comment **Edit comment** and **Delete comment…** actions inside a compact `⋯` popup menu. PR comments, Review submissions, replies, edits (`github.remote.comment.update`: `PATCH` for issue and PR review comments, `PUT` for PR review summaries), and deletions (`github.remote.comment.delete`: `DELETE` for issue comments and PR review thread comments; omitted for submitted PR Review summaries because GitHub REST API does not permit deleting submitted reviews) authored by the active signed-in GitHub user explicitly publish to GitHub; Card comments and Agent mentions remain local. Notification Mark as read and Done are explicit operations. Remote discussion never invokes local `comment.add` or schedules an Agent.

Writes retain an intent/receipt keyed by request key. Replaying a receipt never republishes the action. Publication outcome unknown retains the draft and requires checking GitHub before enabling a new submission; navigation never retries a write.

### Automated PR review publishing and repository-level event polling

- **Decoupled `@reviewer` draft + optional `canban review publish`:** Automated PR review workflows keep `@reviewer` writing a local draft (`review_draft.json`) without direct remote write privileges. An optional, toggleable (`enabled: true/false`) post-review `script` step invokes `canban review publish` (with optional `--event-override COMMENT`), which calls the step-authorized `github.remote.review` path queued through `github_action_gate` and clears `review-draft:{cid}` on success.
- **Repository-level event polling independent of `intake: off`:** When a linked Project has active `github.*` workflow subscriptions, background polling scans repository-level endpoints (`/issues?since=...` filtering out PRs, and `/issues/comments?since=...`) even while Card `intake` is `off`. Durable repository baseline watermarks live in `source_observations` (separate from disposable response caches) so `opened` is emitted only when `created_at >= baseline` while pre-existing objects emit `discovered`.
- **Canonical payloads and cross-GitHub loop suppression:** GitHub events remain body-free (preserving the disposable-cache boundary), use canonical PR/Issue event names and `actor = "github:<login>"`, match cardless Project subscriptions via `selector.repository` without stamping `event.project_id`, and suppress cross-GitHub feedback loops via `exclude_self` plus receipt-tracked remote IDs (`comment_id` / `review_id`) persisted in `source_observations`. See [Events and subscriptions](product/events-and-notifications.md) and [Unified workflow engine](product/workflow-engine.md).

## Cache implementation

AppServer owns the service lifetime and update events. Provider-side response caching uses ordinary files and bounded in-memory maps; no cache database, Redis or separate cache framework is introduced.

```text
UI / CLI -> AppServer -> GitHub provider or gh transport
                         |
                         +-- in-memory response cache
                         +-- <CANBAN_HOME>/cache/github/app/
                         +-- <CANBAN_HOME>/cache/github/gh/

Business SQLite: Cards, bindings, rules, local history, operation receipts
Disposable files: remote bodies, list pages, discussions, checks, diff
```

Cache keys include host, authorization source/account and complete request identity, never tokens. API and CLI scopes stay separate. Files contain a response envelope: body, pagination Link, ETag, Last-Modified, poll interval and last successful validation time. Temporary-file replacement prevents partial writes from becoming valid entries. Private directories/files use restricted filesystem permissions. Corrupt entries are misses. Cache content is excluded from Card archives.

There is **no short freshness TTL**. A cache hit is immediately usable and opening a page does not revalidate it merely because time passed. A miss fetches the requested page. Explicit Refresh, the existing roughly 120-second source/background worker and successful remote actions update cache entries and emit GitHub change events. A cache revision travels over the existing AppClient state observation channel, preserving the GitHub topic even when no Card fields change; no second UI polling loop is added. Notification requests additionally respect GitHub's X-Poll-Interval. Recently visited pages and details are retained as a bounded warm-query list under `cache/github/queries.json`, allowing startup/background refresh without fetching unvisited pages or every repository.

Conditional reads use ETag/Last-Modified when provided; a 304 retains the response body and updates validation time. Concurrent identical requests share a per-key gate. Account/cache generations fence late writes after disconnect or cache clearing; older source snapshots cannot replace newer cached source revisions. Errors retain usable cached content without advancing successful-validation time. Authentication/access errors are surfaced separately from an empty successful result.

Each transport cache is bounded to 32 MiB of serialized in-memory entries and 256 MiB of disk entries. Memory uses access ordering; disk evicts the oldest saved entries. Capacity eviction and explicit cleanup are independent of freshness. The UI does not require an entry to remain present forever. Large response bodies have a 4 MiB transport bound.

Clear GitHub cache preserves Cards, associations, rules, local comments and delivery receipts. Disconnecting the App clears its response cache and credentials, preserving task ownership. CLI credentials remain managed by gh. Cache cleanup does not remove Git worktrees or objects.

## Persistence and migration

Schema 9 extends source uniqueness to PR Cards. The combined release uses schema 10 for durable editor drafts and desktop API 8. Paired migrations also accept the editor branch's previous schema-9 shape; rollback preserves preexisting draft tables. New source snapshots and remote discussion bodies cannot be written through the business storage API. Source titles are persisted as a repository/number placeholder; the current title is projected at read time. Delivery repository/PR/workspace bindings and locally accepted publication checkpoints remain durable; current Checks/Review summaries and remote delivery state come from cache. A confirmed merge is retained as a local delivery receipt.

Upgrade exports old `github_issues` snapshots and imported remote discussion/sync events to `cache/github/legacy/v8.json` before removing their business-table copies. Mirrored source titles in old creation/edit events and Card-shaped operation receipts are also exported and replaced with source placeholders. Local comments, Card identity/stage, reports, execution history and historical archive/input files are retained. Existing captured historical files are not silently rewritten. The migration export is retained separately for inspection and does not masquerade as an account-scoped current response.

New execution manifests keep source identities, local instructions and local history. They do not automatically archive a remote body snapshot. Agents can read the current source through `github.context`; clearing caches or later remote edits means historical remote prose is not guaranteed to be reproducible. Agent-authored reports remain local work history.

## Validation

`mise run test` and `canban-sim` (`github_sync`, `workflow_faults`, `review_and_events`, `chaos`) cover provider caching/ETag/request coalescing, canonical references, pagination isolation, restart, migration, PR delivery, and source ownership. Gallery includes GitHub inbox, PR diff and offline cached-content scenes using production components. These fixtures are not live GitHub protected-branch, permission or OAuth acceptance. Historical live App/PR/merge evidence remains in [GitHub integration](github-integration.md).

The [2026-11-10 acceptance report](github-workspace-acceptance-20261110.md) records the automated and native checks and the boundary between live notification reads and fixture-only writes.

## Repository settings and asynchronous synchronization (2026-11-16)

Repository settings use controlled toggles for Issue/PR intake, Draft PRs and
pausing automatic synchronization. Scope selectors update a local draft directly;
they do not mirror values through bidirectional reactive effects. Disabling intake
retains its draft scope. Save remains explicit. Same-repository rule saves are local
transactions; first/new repository bindings validate identity asynchronously.
Collaborator permissions are checked during synchronization and failures remain visible.

Local remote candidates and GitHub fork/parent metadata load independently off the
UI thread. Selecting a candidate or parent changes only the draft source repository,
never local Git remotes. A fork is not silently redirected to its parent.

`github.sync.request` queues a project and returns immediately. Two workers run
independently of warm browsing-query refresh. Requests for the same generation
coalesce; new settings replace pending requests and fence old results. The existing
`github.sync` RPC remains a synchronous compatibility operation, serialized against
queued work for the same project. Saving schedules one manual synchronization even
when automatic synchronization is paused. Issue and PR stages run independently;
errors from either stage and activity observation are retained, and last-success
advances only when the entire run succeeds.

Ephemeral `Snapshot.github_sync` status is shared by Board/settings and merged across
hosts. Worker status resets at service restart; the saved source retains last-success
and error. UI RPCs, candidate discovery and fork metadata use background execution.
Leaving the page does not cancel synchronization or overwrite unsaved drafts.
Desktop API 61 introduces the queue, discovery RPCs and snapshot status.

[2026-11-16 synchronization acceptance](github-sync-acceptance-20261116.md) records the automated, native and live Fiber source checks and their boundaries.

## Unified Board filters and persistence

The Board uses one compact filter bar on both local and GitHub Projects:
`Type: All cards`, `Author: Anyone`, `Tags`, and `More`. Every filter uses the
same control height and active border treatment. Type combines source and object
kind: All cards, Local cards, GitHub cards, GitHub Issues, or GitHub PRs. Local
Cards with delivery PRs or GitHub references remain Local cards.

Author is one mutually exclusive choice: Anyone, Mine, or a searchable author.
Mine matches the active desktop GitHub account, case-insensitively: local
`created_by` for Local Cards and cached source authorship for Issue/PR Cards.
Importing a PR, assigning it to someone, or running an Agent does not establish
authorship. Without a signed-in account, Mine matches the server's `Local`
creator and explains that GitHub authorship requires sign-in. Legacy `Local`
records cannot be attributed to a signed-in account and are excluded with a
visible explanation. Missing source details never fall back to the importing
user; the filter stays selected and explains the missing metadata.

Tags allow multiple selections (AND). More contains GitHub state and the
independent Scheduled-only condition. Selected tags and More conditions remain
visible as removable chips. Selecting a GitHub state restricts Type to GitHub
when needed; selecting Local or All cards clears the remote-state condition.
Reset clears all conditions for the current Project. Empty filtered results offer
Reset; column counts distinguish visible Cards from totals. Narrow layouts wrap
controls rather than concealing active conditions. Command Palette entries use
the same filter intents.

The local desktop AppServer atomically saves filters in `config/desktop.json`,
scoped by desktop account and host-qualified Project identity. They restore on
Project navigation and desktop restart. Mine stores a relative mode rather than
a literal login. Per-Project patches preserve other Projects, accounts and
preferences. One writer serializes/coalesces rapid edits, retains failed writes
for retry on the next edit, and flushes pending writes on orderly desktop Quit.
These preferences only affect presentation, never Card state or scheduling.

[2026-11-17 Board filter acceptance](board-filters-acceptance-20261117.md) records
the automated checks, native startup observations and remaining interaction checks.

## Large linked Project performance

Board filtering borrows the active identity and uses the shared source index for
GitHub authorship/state. Unrestricted filters skip source lookup. Board geometry
keeps ordered Card indices, combines native height changes once per display frame,
and mounts viewport rows plus overscan instead of every matching Card. The lifted
and focused Card owners remain mounted across scrolling. Status-only snapshots
that leave filtered Cards equal preserve the existing row collection and geometry.

Issue discovery indexes tracked/removed source identities and cached revisions.
Reconciliation builds a Project-scoped source/delivery/append-order index once per
transaction. Workers apply at most 64 source changes per transaction and release
the shared Store between batches. Each batch rechecks account/source generations
and local removals. Earlier committed batches remain if a later batch fails;
last-success advances only after the complete Issues/PRs/observations run succeeds.
Provider cache reads/serialization happen before taking Store, and observation
relations are indexed once per batch. Source deduplication, delivery-PR exclusion,
local instructions, history and removal suppression keep their existing semantics.
