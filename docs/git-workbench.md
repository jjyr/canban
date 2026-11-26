# Git workbench

Current contract, updated 2026-11-19 (Cards #100, #106, #108 and #138).

Git is a checkout-oriented workbench with **Commits / Files / Changes** columns.
The top Worktree picker selects the Project checkout, a managed worktree, or a
Card worktree. Each physical directory appears once, including canonical path
aliases. Selecting a checkout defaults to **Uncommitted**, even when it is clean.
Fetch, Pull, Push, staging and commits act in that checkout. Re-entering Git
retains the selected checkout and comparison for the same host and Project.

Cards are optional context. AppServer matches active Cards by canonical worktree
path on the owning machine; branch names and Board selection do not establish
an association. Exactly one match exposes a compact `#N ↗` link to that Card.
Ambiguous or absent matches expose no Card link. The hint never changes the
comparison or authorizes Card reporting, workflow changes or delivery. The
Worktree picker and Commits column do not repeat Card/PR inventories. Opening a
local Card or managed worktree in Git selects its checkout and Uncommitted view;
remote PR diffs remain available from GitHub without a local worktree.

The Commits column contains one Uncommitted entry followed by searchable commit
history. Uncommitted displays separate **Staged** (HEAD to index) and **Unstaged**
(index to working directory, including untracked files) groups in Files and
Changes. A path modified in both groups has two independently selectable and
foldable patches. This preserves edits that cancel out in the net HEAD-to-disk
diff. Repositories without a first commit use the empty tree as the index base.
Selecting a commit displays its parent-to-commit diff and metadata, including the
existing merge-parent selector.

Custom Base/Compare selectors, swap and merge-base controls are hidden behind
**Compare…**. They retain arbitrary ref, branch and tag comparison support. A
comparison never switches the operation checkout. Choosing another worktree
returns to its Uncommitted view rather than carrying refs from another checkout.
The Worktree picker displays its directory and branch, with the full path on hover.

Changes is a continuous file view. Clicking a file name reveals and expands its
patch without filtering out other files, and each file header exposes a
copy-path icon button beside the selectable filename to copy the full relative
path. Chevron icons in Files and Changes share the same per-patch fold state. A collapsed patch retains its file name,
source group, status and change statistics. Refresh of the same query retains
fold state; changing comparison or checkout resets it. Command Palette entries
expose Compare, checkout selection, related Card navigation and file reveal/fold.

Clicking the Git entry from a Card or Terminal resolves that context's Project;
entering from other pages retains the existing Project. Re-entering the same
Project retains its selected checkout and range. Changing Project clears the
old branches, comparison range, checkout and results before loading the new
refs, fencing late reads. History retains its own Project without entry-point
inference; foreign Git selections are cleared when that page mounts.

The commit-history branch selector, search input and Uncommitted row remain
pinned above the scrollable commit list so asynchronous reads never replace an
open branch popover, clear its query or remove keyboard focus. Commit search is
debounced by 150 ms; Enter requests it immediately. The refs response supplies
the initial commit page and updates the active branch's head commits whenever
fresh refs arrive without a search filter, while preserving already-appended
pagination when HEAD is unchanged. Pulling down at the top of the Commits scroll
view (native trackpad pull-to-refresh or discrete mouse-wheel overscroll at the
top edge) invalidates cached reads for the active checkout and fetches fresh
refs and commits from local Git. Scrolling near the bottom of the Commits list
automatically appends the next 40-commit page when more history exists, with
`Load more commits…` retained as an explicit fallback.

## Direct change navigation

Card #104 extends branch labels into direct Compare entry points. A Card's branch
opens its bound workspace, while a Terminal's branch follows the observed
checkout, including after the Agent changes directory. Neither opens a workspace
switching dropdown. Local Card and managed-worktree entries default to Uncommitted. Explicit
Terminal range actions retain their observed checkout and range; a Project root
compares HEAD with Working tree. These views include staged, unstaged and
untracked files. Worktree management stays in the Worktree picker, Terminal tree
context menus and the Command Palette.

The visible Card header summarizes delivery changes against its bound target
branch in its checkout. This is independent of Git's default Uncommitted view.
It shares the window's bounded read cache and in-flight request coalescing. No Board-tile Git queries or polling timers are
added. Unrelated Snapshot changes do not reload statistics. Known review events,
host recovery, explicit Refresh Card Diff and reopening after cache expiry reread
the relevant scope. Query tickets discard late results after a Card/context
switch; unavailable hosts and errors never appear as zero-change results.

Header counts summarize the rendered patch; a truncated patch is labelled `≥`
and explains that the counts are partial. Empty, loading, unavailable and missing
workspace are distinct states. Binary-only changes remain discoverable through
the tooltip's file count even when line totals are zero. Card context menus and
the Command Palette provide branch copying and diff refresh. Ordinary Card open
still collapses Terminal; returning directly from Compare restores the prior
split and selected live tab without replaying execution. Drafts and Card reading
positions continue using their existing route owners.

Gallery scenes: `card-changes`, `card-changes-loading`, `card-changes-error`,
`card-changes-empty` and `card-changes-missing` use the production header with
deterministic data. `terminals` exercises the direct branch action and existing
worktree tree. Gallery states are distinct from packaged-App acceptance. See the
[Card #104 acceptance record](card-changes-acceptance-20261117.md) for executed
checks, build-tool workarounds and unexercised boundaries.

## Asynchronous reads and cache

Each window owns a capacity-bounded LRU of 24 Git read results with a ten-second
reuse interval. Keys include host, Project, method and complete query parameters
(including checkout, Card, range, merge-base setting, search and pagination).
RPC calls and patch-line parsing run on the blocking pool. Concurrent identical
reads share one request and prepared result. Failed reads are not cached.

Invalidation follows the event's owning host. A connection-status change does
not invalidate Git data. Actual host recovery (`resync`) invalidates only that
host's reads; known Card `review` events narrow this to its Project/worktree.
Manual refresh and Git mutations invalidate the active checkout. Unknown event
ownership falls back to the originating host, never every host. Hidden-page
caches follow the same rules and re-entry reads again. Incompatible remote
daemons cannot refresh a local Git view, even when they recover successfully.

Invalidation detaches matching in-flight reads, preventing older completions
from refilling the cache or replacing a newer flight. Per-query tickets prevent
older responses from replacing the current view after a context/range change.
Repeated refresh requests during a read batch queue at most one trailing batch,
allowing the current reads to finish. Changing context/range drops the old pending
refresh. External filesystem changes without an event are observed on the next
expired read or explicit refresh. An initial refs response must not overwrite a
range edited while it was loading.

Loading state is separate from the code content. Only an unresolved initial
query shows the full-size loading placeholder. Refreshing an existing result,
including an empty result, retains the code scroll container and current content.
Unrelated Snapshot/connection updates do not rebuild code rows; account identity
and working-tree status use equality-filtered projections. Commit-history
navigation updates are confined to the commit header. Switching queries clears
mismatched content. An offline remote view retains known content, shows the host
notice, and waits for recovery rather than repeatedly issuing Git reads.

Prepared patch lines and parsed unified hunks are shared with rendering and keyed
by source group plus path so file expansion never re-parses patches on the UI
thread. Changes initially mounts four patches and automatically expands subsequent
batches as the user scrolls near the bottom of the Changes viewport (`Show more
files` remains available as an explicit button and Command Palette action).
Selecting a later file reveals intervening headers with their unseen bodies
folded, then expands and scrolls to the target. Already mounted patches retain
their state. Expanded files render their complete diff hunks without mid-file
line truncation; generated/lockfiles (such as `Cargo.lock`, `package-lock.json`,
`pnpm-lock.yaml`, `*.min.js`) and extreme single-file diffs (> 5,000 diff lines)
start collapsed by default with a `Load diff` prompt, rendering in full once
expanded.

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

`CANBAN_GALLERY_SCENE=review-refresh mise run gallery` changes diff/refs loading
and unrelated Snapshot metadata every 800 ms. Open an inline comment, type a
draft, wait through several updates, then continue typing without refocusing.
The editor, draft and focus must survive; code must not return to Loading.

`CANBAN_GALLERY_SCENE=review-large-diff mise run gallery` contains 60 files with
400 changed lines per file. Verify bounded initial content, continuous file reveal, per-file folding and Show more files. The
`review-uncommitted` scene adds the same path in both Staged and Unstaged and a
matched Card shortcut; fold each source independently from either column. `review-working-tree` and `review-commits` retain
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

## Host-scoped refresh regression acceptance (2026-11-16)

Validated from base `c17019f` plus the refresh-isolation changes:

- `mise run test`: 573 passed, one intentionally skipped. Regressions cover
  repeated incompatible probes, a single host-scoped recovery event, host/Project/
  checkout cache isolation, invalidated flight completion, retained code nodes
  across loading/Snapshot changes, unrelated host/Project/worktree events, and a
  burst draining into one trailing refresh.
- `mise run acceptance` passed with disposable home
  `/private/tmp/cb-k92mtxzj/home` and an isolated Core; `mise run build` packaged
  `.local/Canban.app`. No RPC/schema change or desktop API bump was needed.
- The packaged native fixture under `/private/tmp/cb-git-refresh-neb4kbca`
  used a disposable repository, isolated home/Core and local TCP mock
  remote. API 60 versus App API 61 reproduced the original mismatch. Across
  repeated mismatch probes the local Git reads stayed at one refs and one compare.
  One explicit refresh increased refs/compare to two and log to one. Subsequent
  remote recovery and forced reconnection into mismatch did not increase these
  counts. The code remained scrolled to line 10 during and after refresh/recovery.
  Method counts and assertions are recorded in the fixture's `evidence.json`.
- The production `review-refresh` Gallery alternated loading and unrelated
  Snapshot metadata every 800 ms. An inline comment draft survived these updates;
  after explicitly focusing its editor, later text was appended without
  refocusing. The native code area remained visible throughout.
- Changed-file formatting and `git diff --check` passed. Optional strict Clippy
  checks are blocked by existing lints in unchanged files: test-module ordering
  in `crates/ui-kit/src/button.rs`, needless borrows in
  `crates/app-server/src/github.rs`, and `chunks_exact_to_as_chunks` in
  `crates/profiles/src/launch.rs`. These are separate from the passing runtime
  tests; no unrelated source was changed to silence them.
  Production libraries passed
  `cargo clippy --workspace --lib -- -D warnings -A clippy::needless_borrow`,
  allowing only the known pre-existing GitHub borrow lint. Native fixture and
  Gallery processes and their isolated services/Core were closed afterwards.

## Simplified workbench acceptance (2026-11-17, Card #106)

Validated on macOS from base `7059480` plus the workbench changes:

- `RUSTUP_AUTO_INSTALL=0 bu exec -- mise run build` packaged and signed
  `.local/Canban.app`. The desktop API is 63 for the checkout Card hint and
  `UNCOMMITTED` comparison/source-group payloads.
- `mise run test -- --no-fail-fast`, through Bullet, passed 583 tests with one
  configured skip. An initial run exposed eager `ElementRef` construction in
  window-independent state initialization; references now belong to the mounted
  diff viewport. The full rerun includes a direct initialization regression,
  the two previously failing Terminal tests, source-specific prepared patches,
  independent folding, canonical/ambiguous worktree matching, cancelling index/
  disk edits and repositories without a first commit.
- `mise run acceptance`, through Bullet, passed with isolated home
  `/private/tmp/cb-8ldzcg2r/home` and isolated Herdr Core. This is the deterministic
  PTY/workflow suite, separate from the native Git checks below.
- Packaged native Git acceptance used a disposable source repository, local bare
  remote, Canban home and Core under `/private/tmp/cb-git106-9fgcn1sb`. Project and
  standalone checkouts displayed clean Uncommitted views without a Card link;
  selecting the Card worktree exposed one `#1` link. Opening that link and Back
  restored the Git workbench without changing its checkout. Both staged and
  unstaged versions of `mixed.rs` showed their own exact patches. Chevrons in
  Files and Changes synchronized independent fold states; refreshing the same
  comparison retained a folded file.
- Native single-file Stage and Unstage were verified against the real index and
  untracked status. Commit & Push created
  `d58b43a72f4f99e6a5250917cdf060fefe791880` in the selected worktree and local bare
  remote, while preserving the unstaged disk edit. Stage All followed by Amend
  produced `73484a114c5f8284fddf2f6beac1d67ec7277db0`, with two total commits and a
  clean worktree. The Project source HEAD stayed at its seed commit and the
  associated Card remained Backlog at revision 1.
- Native history inspection showed the amended commit's three files and parent
  diff. Compare… selected `main` as Base and retained the selected commit as
  Compare, displaying the same three-file range without changing checkout.
- The production `review-uncommitted` Gallery verified the same path's two
  source groups and folding from both columns. The 60-file, 400-line-per-file
  `review-large-diff` Gallery revealed `src/module-59.rs` in the continuous view,
  retained earlier patches and folded intervening unseen bodies. This establishes
  bounded construction/navigation behavior, not a quantitative frame-time result.
- A later Gallery rebuild encountered Bullet 0.2.0's prebuilt-toolchain bootstrap
  HTTP 404 before compilation. Native Gallery checks used the already packaged
  production Gallery; the application package and full test/acceptance runs above
  had succeeded. After explicit user approval, duplicate checking confirmed the
  download failure is already tracked by bullet Card #8 (`0ffbe7a2`); no duplicate
  Card, toolchain installation or native-Cargo fallback was performed.

Native check receipts and Git results are in the temporary fixture's
`evidence.json`, with `before-commit.png`, `commit-comparison.png` and
`large-reveal.png` as supplementary UI evidence. Historical test counts above
remain evidence for their original revisions, not this change. No prompts or
workflow YAML were modified.

The contextual Project routing and history acceptance for Card #108 is recorded
in [Project navigation acceptance, 2026-11-17](project-navigation-acceptance-20261117.md).
Card #106 supersedes its local Card/worktree default range with Uncommitted;
its cross-Project isolation and observed Terminal context rules remain in effect.

## Main integration verification (2026-11-17, Card #106)

Rebased the workbench change onto `fcaee750` after the automatic merger stopped
on a provider quota error. Resolved the five conflicts while retaining contextual
Project routing, foreign-result fencing, observed Terminal checkout navigation,
Card summary invalidation and history checkpoints. Same-Project Git entry retains
its selected worktree/range; local Card and managed-worktree entry defaults to
Uncommitted. The navigation regression covers both retention and Project changes.

- `RUSTUP_AUTO_INSTALL=0 bu exec -- mise run build` passed and packaged the App.
- `RUSTUP_AUTO_INSTALL=0 bu exec -- mise run test` passed 591 tests, with one
  configured skip. `mise run acceptance` through the same Bullet wrapper passed
  with isolated home `/private/tmp/cb-ky7ill4j/home` and isolated Core.
- The rebuilt native App used a fresh disposable repository/home/Core under
  `/private/tmp/cb-git106-rox3hk_b`. Clicking the Card branch opened its worktree's
  Uncommitted view with independent staged and unstaged `mixed.rs` patches.
  Folding its staged patch from Files changed both matching chevrons to Expand
  and removed only that patch body. Global Back returned to the original Card.
  The source HEAD and Card Backlog/revision 1 were unchanged. Temporary processes
  were closed afterwards. This integration check did not repeat the earlier
  native Commit & Push/Amend sequence documented above.
- Changed-file formatting and `git diff --check` passed. A workspace-wide format
  check also found existing differences outside this change; those files were
  left untouched. No prompts or workflow YAML were changed.

## Progressive auto-expand, full diff & Commits refresh acceptance (2026-11-19, Card #138)

Validated on macOS from the Card #138 worktree:

- `mise run test -- --no-fail-fast`: 690 passed, 2 configured skips. New unit and
  mounted-Core UI regressions cover full >800-line file rendering without mid-file
  truncation, default auto-collapse of lockfiles/generated assets and >5,000-line
  extreme diffs, automatic file expansion when scrolling near the bottom of Changes,
  automatic commit pagination when scrolling near the bottom of Commits, pull-to-refresh
  (`EventKind::Refresh`) on the Commits scroll view, and fresh `git.refs` synchronization
  of `log_commits` without discarding appended pages when HEAD is unchanged.
- `mise run build-gallery`, `mise run build` and `mise run acceptance` passed with
  disposable home `/private/tmp/cb-q2ldggbh/home` and isolated Herdr Core (`0.1.18`).
  No RPC/Snapshot/YAML schema changed, so `DESKTOP_API` needed no increment.
- Production `CanbanGallery.app` (`review-large-diff`) verified 1,200-line added file
  rendering without truncation, default-collapsed `Cargo.lock` with one-click expand/collapse
  and `Load diff` prompt, automatic loading of commit history item `#60` when scrolling
  the Commits column, and automatic expansion past the initial 4-file limit when scrolling
  the Changes column.
- Packaged native `Canban.app` acceptance used an isolated home, Core and disposable
  55-commit repository under `/private/tmp/cb-git138-8blpj198`. Verified `Cargo.lock`
  auto-collapsed by default, 1,050-line `big_new.rs` rendered without an 800-line cap,
  scrolling Commits revealed commit `#1` automatically, scrolling Changes auto-mounted
  remaining files through `mod_6.rs`, and creating an external CLI commit `#56` updated
  the Commits list on refresh. All temporary UI/daemon/Core processes and scratch directories
  were stopped and removed afterwards.
