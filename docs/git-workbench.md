# Git workbench

Current contract, updated 2026-11-16 (Card #100).

The left Workspace selector chooses the Project checkout, a managed worktree,
or a Card. Its choice determines the working tree, index and destination of
Fetch, Pull, Push, staging and commits. The top toolbar's Base and Compare
selectors choose the comparison range. Switching a checkout preserves an
explicit range; `HEAD`, Working tree and Staged resolve inside that checkout.
Opening a Card intentionally selects its review context. Choosing either ref
explicitly enters an ordinary Git comparison. The toolbar shows the operation
branch, with the checkout's full directory available on hover.

The commit-history branch selector and search input are retained independently
of the changing Workspace/Card rows and commit results. Receiving refs, a new
commit page or loading state must not dismiss an open branch popover, clear its
query or remove keyboard focus. Commit search is debounced by 150 ms; Enter
requests it immediately. The refs response supplies the initial commit page,
avoiding an identical initial `git.log` query.

## Asynchronous reads and cache

Each window owns a capacity-bounded LRU of 24 Git read results with a ten-second
reuse interval. Keys include host, Project, method and complete query parameters
(including checkout, Card, range, merge-base setting, search and pagination).
RPC calls and patch-line parsing run on the blocking pool. Concurrent identical
reads share one request and prepared result. Failed reads are not cached.

Manual refresh, Git mutations and `review`/`resync` events invalidate the cache.
An invalidation generation prevents older reads from refilling it. Per-query
tickets prevent older responses from replacing the current view after a context
or range change. Events also invalidate cached reads while the page is hidden;
re-entry reads again. External filesystem changes without an event are observed
on the next expired read or explicit refresh. Existing content remains visible
during a refresh of the same query; changing query clears mismatched content.
An initial refs response must not overwrite a range edited while it was loading.

Prepared patch lines are shared with rendering. All files initially mounts four
file bodies; Show more files adds four, and selecting an individual file opens
it directly. The existing per-file 500-line display bound remains in place.
This bounds initial view construction without fetching or parsing on the UI
thread. Refresh and Show more files also appear in the Command Palette.

## Sync feedback

Fetch, Pull and Push mark pending before dispatching their asynchronous RPC.
The adjacent Refresh control uses the shared native rotation animation until
the operation and subsequent Git reads finish. Reduced-motion preferences use
the same busy label and disabled state without rotation. Both Refresh controls
share the state. Concurrent sync actions are disabled; completion and errors
are visible in the existing notification/error surfaces.

Sync completion refreshes Git data for the original active context. It does not
explicitly fetch and replace the complete application snapshot. Navigating to
another checkout during an operation cannot let its response overwrite that
checkout's data. AppServer events continue to deliver durable changes through
the normal application subscription.

## Regression scenes

`CANBAN_GALLERY_SCENE=review-async mise run gallery` uses the production Git
screen with refs, commits and loading changing every 800 ms. Open the history
branch popover, type a query, wait through multiple updates, then select a ref
or dismiss with Escape. The query and focus must survive the updates.

`CANBAN_GALLERY_SCENE=review-large-diff mise run gallery` contains 60 files with
400 changed lines per file. Verify bounded initial content, direct file
selection and Show more files. `review-working-tree` and `review-commits` retain
the staging and commit-inspection compositions. Gallery fixtures do not prove
real Git operation acceptance; use a packaged App with a disposable repository,
local bare remote, isolated Canban home and isolated Herdr Core for that check.

## Acceptance evidence (2026-11-16)

Validated on macOS from base `d2572f49d2ec` plus the Card #100 changes:

- `mise run build` packaged and signed `.local/Canban.app` successfully.
- `mise run test`: 564 passed, one intentionally skipped. After final UI
  refinements, `mise run test -- -E 'package(canban-ui)'`: 152 passed. The new
  cache regressions cover reuse/expiry and key isolation, in-flight invalidation,
  failure retry and concurrent sharing of prepared patch lines.
- `mise run acceptance`: passed with disposable home
  `/private/tmp/cb-bltbt7ni/home` and isolated Herdr Core.
- The production `review-async` Gallery kept its history branch popover, typed
  `main` query and input focus across repeated 800 ms refs/log/loading updates;
  selecting the result succeeded afterwards.
- The production `review-large-diff` Gallery opened 60 files of 400 changed lines
  each. Direct selection opened `src/module-59.rs`. Returning to All files showed
  `Show more files (56 remaining)` at the end of the initial four bodies;
  activating it exposed `src/module-4.rs` while retaining the scroll location.
- Packaged-App native fixtures used disposable repositories and local bare
  remotes under `/private/tmp/cb-git100-5ot633ml` and
  `/private/tmp/cb-git100-4nfxzpah`, with separate Canban homes and Core sockets.
  Three-second remote hooks/upload-pack delays exercised a successful Push,
  fast-forward Pull and rejected Push. During sync the refresh controls rotated,
  sync buttons were disabled and the history popover remained usable. Completion
  stopped the busy state, and rejection displayed the remote error. Source and
  remote refs matched after Push; the pulled commit and file were verified on
  disk. The final packaged build repeated the successful Push/ref check.
- Selecting the managed `worktree-test` checkout preserved `main` and `feature`
  as the explicit comparison pair, while the operation branch changed to
  `worktree-test`. Returning through Back after cache expiry displayed an external
  edit to the working-tree fixture. The latter was confirmed visually because
  individual diff text is represented by comment-action buttons in the native
  accessibility tree.
- Changed-file formatting and `git diff --check` passed. Test UI instances and
  their isolated services/Core were closed afterwards; the installed app and
  real repositories were not used for Git mutations.

These checks establish native interaction and bounded initial construction, not
quantitative frame-time improvements across every repository or remote host.
