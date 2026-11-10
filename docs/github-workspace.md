# GitHub workspace and disposable cache

Status: implemented in the GitHub workspace branch, 2026-11-10. This is the current contract for GitHub navigation, source Cards, browsing and caching. It supersedes the earlier Issue-only intake, Settings entry and permanent remote-comment Timeline imports. Dated live acceptance in [GitHub integration](github-integration.md) remains historical evidence, not acceptance of the new UI.

## Navigation and account access

The primary rail places GitHub immediately above Archive. GitHub contains Notifications, Issues and Pull requests. Account/access and cache controls belong to its Account & access page. Settings no longer duplicates GitHub configuration.

The Board header's GitHub icon opens the selected Project's repository and sync rules. A connection is explicit; a Git remote alone does not enable automatic intake. Existing projects keep their Issue intake mode; PR intake defaults to off. A Project's ordinary local work and execution location remain independent of its GitHub source.

Canban App authentication remains available through the existing Device Flow and private credential file. Repository operations prefer that account when connected. With no App connection, the already signed-in GitHub CLI account supplies repository browsing, intake and explicit API actions. Canban invokes `gh api` without extracting or copying its credentials. Git push continues to use the saved execution machine's Git credentials.

Notifications use GitHub CLI authorization. App and CLI account names must match when both are in use. The page identifies App repository access versus CLI access; neither promises access beyond the selected authorization. App permissions still govern its actions: Issue comment publication requires Issues write permission, which the originally registered App did not request. CLI fallback has the permissions of its own authorization. A failed API action never silently switches accounts.

## Card ownership and references

Cards have Local, GitHub Issue or GitHub PR content. Each source Card has one stable repository/object identity, separate from its local Card number. A Project has at most one source Card per remote object. Source title/body/state are cache-backed projections; editable instructions, stage, execution history and human decisions remain local.

All Card kinds support explicit Issue, PR and commit URL references. Pasting and adding a reference keeps a Local Card local. References open the common GitHub detail workspace. Source, related reference and delivery PR are different relationships. An existing delivery PR opens its owning Card and is excluded from duplicate automatic PR task creation.

The unified Add Card form supports Local, GitHub Issue and GitHub PR. Search/select/preview precede Add; manual addition includes closed sources and is independent of intake eligibility. Creation and restoration use the shared numbering/lifecycle transaction. Removed sources stay excluded until explicitly restored. Active or uncertain execution still prevents removal.

## Intake and Board filters

Issue intake supports off, all Open Issues and collaborator-authored Open Issues. PR intake supports off, all Open PRs, collaborators, assigned to me, authored by me or Review requested from me. Additional author and Label lists narrow new intake, and Draft inclusion is explicit. Author matching is case-insensitive; collaborator membership uses GitHub user IDs. Label alternatives are OR; distinct condition groups are AND.

Automatic intake creates Backlog records without Agent, Session or worktree launch. Changed eligibility never ejects existing Cards. Turning intake off continues to refresh already tracked sources. Pause stops the Project's worker; independent GitHub browsing can still refresh the shared remote cache. Closing an Issue or merging an ordinary related PR does not move or archive a Card. A delivery merge records a local delivery receipt through the existing workflow policy; Archive remains separate.

The Board filter bar supports All/Local/GitHub, Issue/PR type, author and remote state. Local means source kind, including Local Cards with GitHub references. Author means local creator or remote source author, never an Agent Profile. Filters are remembered per Project within the window and affect visibility only. Column counts distinguish visible Cards from their total. Filtering does not change ordering, scheduling or human approval.

## Remote lists and PR workspace

Lists use remote filtering and pagination. Notifications request 50 items per page; Issue/PR search and detail collections request 30. Previous/Next comes from the API Link header, with no fabricated page count. Browsing does not create Cards. Existing source/delivery associations open the matching Card, including archived Cards; otherwise Create sync Card selects a connected Project, or directs the user to repository configuration. Multiple matching Projects require an explicit target.

Remote query pages are cached independently by authorization, endpoint, complete query and page. Cached summaries are not a complete local index and are never assembled into a pretend remote page. List scroll positions and query/page state survive navigation away and back within the window.

Changing a list filter resets it to page 1. A successful explicit first-page refresh invalidates its cached sibling pages because membership and ordering may have changed. Per-query epochs prevent older in-flight later-page reads or refreshes from restoring those invalidated entries; unrelated queries remain cached.

The common detail view offers Overview and Discussion; PRs add Files changed, Reviews, Review threads and Checks. Missing/binary patches have an explicit GitHub fallback. Viewing a PR does not create or change a checkout. GitHub PR diff and the existing local workspace Review diff remain separately labelled views.

PR file pages are cached under both comparison SHAs. On a miss or explicit refresh, Canban checks the PR before and after fetching the files. A moving comparison is rejected rather than cached under the wrong version. Line comments choose the file, side and line number and collect into a pending Review. A Review contains the displayed head SHA, and submission revalidates it against GitHub. New commits do not silently re-anchor a draft.

Comment and Review forms expand on demand so the diff retains its reading area. PR comments, Review submissions and replies explicitly publish to GitHub; Card comments and Agent mentions remain local. Notification Mark as read and Done are explicit operations. Remote discussion never invokes local `comment.add` or schedules an Agent.

Writes retain an intent/receipt keyed by request key. Replaying a receipt never republishes the action. Publication outcome unknown retains the draft and requires checking GitHub before enabling a new submission; navigation never retries a write.

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

Schema 9 extends source uniqueness to PR Cards. New source snapshots and remote discussion bodies cannot be written through the business storage API. Source titles are persisted as a repository/number placeholder; the current title is projected at read time. Delivery repository/PR/workspace bindings and locally accepted publication checkpoints remain durable; current Checks/Review summaries and remote delivery state come from cache. A confirmed merge is retained as a local delivery receipt.

Upgrade exports old `github_issues` snapshots and imported remote discussion/sync events to `cache/github/legacy/v8.json` before removing their business-table copies. Mirrored source titles in old creation/edit events and Card-shaped operation receipts are also exported and replaced with source placeholders. Local comments, Card identity/stage, reports, execution history and historical archive/input files are retained. Existing captured historical files are not silently rewritten. The migration export is retained separately for inspection and does not masquerade as an account-scoped current response.

New execution manifests keep source identities, local instructions and local history. They do not automatically archive a remote body snapshot. Agents can read the current source through `github.context`; clearing caches or later remote edits means historical remote prose is not guaranteed to be reproducible. Agent-authored reports remain local work history.

## Validation

`mise run test` covers provider caching/ETag/request coalescing, canonical references, pagination isolation, restart, migration and source ownership. `mise run acceptance-github-workspace` runs the real AppServer and gh subprocess boundary against `scripts/fixture-github.py` in an isolated home/Core. It checks Issue/PR intake, local references, paginated notification cache, versioned diff, write receipt replay, stale-head rejection, source refresh without dispatch and absence of remote bodies in business tables.

`mise run acceptance-github-native` adds the packaged native app against the same deterministic remote for interactive acceptance. Gallery includes GitHub inbox, PR diff and offline cached-content scenes using production components. These fixtures are not live GitHub protected-branch, permission or OAuth acceptance. Historical live App/PR/merge evidence remains in [GitHub integration](github-integration.md).

The [2026-11-10 acceptance report](github-workspace-acceptance-20261110.md) records the automated and native checks and the boundary between live notification reads and fixture-only writes.
