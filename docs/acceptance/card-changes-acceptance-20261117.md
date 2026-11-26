# Direct Card and Terminal change navigation acceptance

Validated on macOS from `c8deed42908f29512b386a92d76e17be8d56be1b` plus the
Card #104 implementation. This extends the existing Git workbench; no RPC,
Snapshot, database, workflow or prompt schema changes were introduced.

## Implemented behavior

- Terminal's branch action directly opens the observed checkout's comparison.
  The former Project/Worktree dropdown and its Idle Managed Worktrees group
  are removed. The left tree still selects active workspaces, while Git's
  Workspace list and the Command Palette retain idle-worktree discovery.
- Card's branch action displays the default comparison's additions/deletions
  and opens the bound Card comparison. The visible header shares Git's bounded
  read cache and request coalescing; no Board-tile queries or polling were added.
  Copy and Refresh live in its context menu, Card menu and Command Palette.
- Managed worktrees default to base branch through Working tree, including
  staged, unstaged and untracked changes. Project roots use HEAD through Working
  tree. A Card keeps its bound comparison target even when its terminal changes
  directory. Terminal navigation follows the new observed directory.
- Returning directly from Compare restores the Card's selected live terminal
  and split. The explicit Back to Card action and global Back use the same
  restoration. Navigation captures source/destination synchronously to avoid
  skipping the Card before the deferred history watcher catches up. Ordinary
  Card open and Board-history return retain their collapsed-terminal behavior.

## Automated checks

- `RUSTUP_AUTO_INSTALL=0 bu exec -- mise run test -- --test-threads 4`:
  575 passed, one intentionally skipped.
- Final affected-suite check after the visible-review-drawer observer adjustment:
  `mise run test -- -E 'package(canban-ui)' --test-threads 4` through Bullet:
  161 passed.
- `mise run build` through Bullet packaged and signed `.local/Canban.app`.
- Final `mise run acceptance` through Bullet passed with disposable Canban home
  `/private/tmp/cb-tf64jf7x/home` and its isolated Herdr Core.
- Changed-file rustfmt checks and `git diff --check` passed.

The initial full run caught the overly broad Terminal-history restoration;
this was restricted to Compare return and regressed against both behaviors.
Another full run, while other builds were active, hit the existing background
merge test's five-second push-start deadline. The complete bounded-concurrency
rerun passed; no test was skipped to obtain the result.

Bullet's isolated outputs initially prevented the packaging script from finding
`target/debug/canban-ui`; `bu path` selectors also failed to find an active
`.live` output. These match existing bullet Project Cards #1 and #2; duplicate
Cards were not filed or modified. The final built binaries were explicitly
copied from the observed Bullet cache into this checkout's ignored staging
paths before running the unchanged packaging task. Empty staging files from
the initial failed packaging attempts were removed after verifying their size.
No shared toolchain, Cargo, Bullet, or packaging configuration was changed.
The packaged UI and the final native fixture UI had identical SHA-256:
`3fc9613b69a3297a024424e2b764587e7bfd18b5b0f5af250fe022ae2d5cf644`.
Packaging replaces the UI's code signature, so its raw hash differs from the
unsigned/linker-signed build-cache input.

## Native interaction checks

The final production App fixture used disposable Git repositories, a separate
Canban home and isolated Herdr Core under
`/private/tmp/cb-changes104-v1qpr8et`. No real Agent was launched. The earlier
fixture `/private/tmp/cb-changes104-7y3an48d` exercised the same comparison paths
before the history fix. Temporary fixture launch code and metadata are outside
Git (`/private/tmp/canban104-native.py`, `native.json` in each fixture).

Observed through cua-driver against the exact fixture windows:

- Card header showed `+4 −1`; clicking its branch opened three files with the
  same totals. One committed modification, staged file, unstaged line and
  untracked file were included. A later target-only commit was excluded.
- A comment draft and expanded Terminal 1 survived both Back to Card and global
  Back. Neither path started another terminal. Direct native checks found the
  original history race and then passed on the final packaged binary.
- Right-clicking the branch opened View Card changes in Git, Copy Card branch,
  and Refresh Card diff. After appending an untracked line on disk, Refresh
  updated the visible header from `+4 −1` to `+5 −1`.
- Managed-worktree navigation displayed two files and `+3 −1`, including an
  untracked file. Project-root navigation displayed only its untracked file,
  `+1 −0`, using HEAD through Working tree.
- The original Card shell was changed into the managed worktree via the scratch
  Core. Selecting that same terminal and clicking its branch opened
  `feature/managed104`, its untracked file and `+3 −1`, rather than the original
  Card comparison.
- A non-Git directory displayed the disabled branch action. The driver's
  `enabled:false` assertion could not address the disabled AX row; its fresh
  accessibility tree showed the action without a press operation. No attempt
  was made to force activation.
- Git's Workspace selector still listed the idle `feature/idle104` worktree.
- Production Gallery scenes `card-changes` and `card-changes-error` displayed the
  long mixed-language branch with correct totals or Diff unavailable. The branch
  action navigated directly to the existing Git view. Loading, empty and missing
  workspace scenes were added as deterministic regression states.

Screenshots are retained under `/private/tmp/canban104-*.png`, including
`canban104-final-return.png`, `canban104-refreshed.png`, and
`canban104-gallery-error.png`. Test logs use `/private/tmp/canban104-*.log`.
All task-owned native/Gallery processes and scratch Core services were closed;
the installed App, live Core and source repositories were not used for Git
mutations.

These checks establish local navigation, default ranges, explicit refresh,
draft retention and live-terminal restoration. Native reader-scroll retention,
remote-host recovery, all Gallery variants, PR-only and archived-workspace
interaction were not separately re-exercised in this run. Existing draft/scroll,
remote routing, PR and archive mechanisms remain in use.
