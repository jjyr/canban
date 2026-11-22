# Event-driven runtime observation

Implemented 2026-11-09. This supersedes the proposed periodic reconciliation in [the cost assessment](runtime-observation-cost.md). Canban no longer runs a half-second Terminal inventory scan or individual Agent status loop. GitHub synchronization and the remote Agent reporting bridge are separate responsibilities.

The subscription below is **Herdr Core -> AppServer** runtime observation.
Desktop **AppServer -> UI** synchronization has its own boundary: local AppClient
waits on a lightweight `app.watch` cursor; remote-host supervision makes
conditional `app.state` requests. Changed cursors can still lead to full Snapshot
refreshes. See the [UI data/ownership contract](ui-architecture.md#state-synchronization-ownership-and-memory-cost)
and [memory profiling implementation and evidence](memory-profiling.md); this
runtime subscription is not a claim of per-entity incremental desktop updates.

```text
Herdr Core (local terminals + remote mirrors)
       |
       | one subscription: initial snapshot, events, missed, heartbeat
       v
canban-herdr::events
       |
       v
AppServer runtime inventory --------> state transitions / durable history
       |                                      |
       +--> directory context queue           +--> UI / Scheduler wake
                  |
             global maximum 4 checks
             one check per machine
                  |
             shared directory caches

Card / report / question / configuration changes --> Scheduler wake
idle deadline / close timeout / queued retry ------> Scheduler wake
```

## Subscription and recovery

`canban-herdr` owns a dedicated plain JSON-RPC event connection, independent of PTY output, rendering and viewer leases. Setup reads Core identity and machine routes, then subscribes and consumes the returned sequence/snapshot. No per-Terminal attachment is created for status observation.

AppServer applies `created`, `restarted`, `status`, `turn`, `foreground`, `finished`, `lost`, `removed` and relevant label changes to its shared inventory. Routes are normalized to immutable machine-owner IDs. Duplicate/older sequence numbers are ignored; sequence gaps alone do not imply loss because filtered events can skip sequence numbers.

`missed` replaces the inventory with its supplied snapshot. Disconnect, subscriber overflow, configuration changes and 45 seconds of silence invalidate the connection and cause a reconnect with exponential backoff (1–30 seconds). A new subscription supplies a fresh snapshot and resets directory discovery. No periodic Terminal list/get scan is used to recover. Transport loss and an unavailable remote mirror are not confirmed process exits. A locally authoritative absent record is Missing, with no invented exit code; previously confirmed exit information remains intact.

Heartbeats update connection freshness in memory. Snapshot projections expose this freshness without rewriting every Attempt or generating Timeline events on every heartbeat. Ordinary lifecycle changes still produce durable history, and the DB is never held during Git/network discovery.

### Fine-grained screen status refinement

Some TUI agents finish their foreground turn and report `idle`/`done` to Herdr while background work inside the same session is still running. `canban-herdr::refine` provides per-agent screen refinement in the adapter layer, starting with Antigravity (`agy`):

- **Trigger scope**: AppServer only considers live terminals recognized as `agy` (`antigravity`) whose Herdr `status.state` is `idle` or `done` (never `working`, `blocked`, or `exited`, so permission gates and confirmed exits always retain priority).
- **Bottom-chrome inspection**: On a qualifying transition, AppServer calls `terminal.read` (`lines: 0`, current visible screen without scrollback) and `canban-herdr::refine::agy` inspects the bottom TUI active-items tray and footer below the `>` prompt rules (plus the full-screen `/tasks` overlay). Conversation transcript and text typed inside the prompt box are ignored.
- **Refined states**:
  - Running background tasks and schedules (`● ... running` in the tray, `[1-9]\d* task(s) · /tasks` in the footer, or `<N> running, <M> done` with `N > 0` in `/tasks`) refine `status.state` to `"working"` (`status.raw_state` preserved, `status.refined = "agy.background_task"`). Swarm and subagent tray/footer rows are intentionally not refined from screen text.
- **Caching and bounded re-checks**: When a screen probe returns `Keep` (`idle`/`done`), the result is cached by `(core_id, address, raw_state, turn, status_ms, reported)` with no follow-up deadline — genuinely idle terminals perform zero repeat `terminal.read` probes. Only while a terminal is actively refined to `"working"` does AppServer schedule a bounded re-check (`2.5s`) through the existing Scheduler deadline loop, and conditional idle-close / close-timeout safety checks re-verify refinement immediately before closing.

## Scheduling and deadlines

Runtime observation, request creation, reports, immediate questions, human approval and configuration changes wake a coalesced service worker. Admission preserves the configurable Agent limit, workflow depth priority and human gates. A launch receipt wakes admission again so multiple available slots fill without a half-second delay between launches.

Idle reclamation and close timeouts use the nearest outstanding deadline. Human terminal activity wakes the worker and updates the effective idle deadline. A retained Agent can become active again and later idle again; merely viewing it does not protect it indefinitely. Eligible queued admission that cannot proceed retries with bounded exponential backoff. With no eligible queue or outstanding deadline, the worker blocks waiting for an event.

Queries immediately before a real action (for example, conditional idle close) still confirm its original binding and current state. Those safety checks are not background scans. AppServer startup and execution Resume retain the original Core/directory boundaries.

## Directory and Git cache

AppServer coalesces discovery by observed cwd. Multiple Terminals in that directory share a result. Targets retain their own Core/Terminal identity, current cwd and mapping generation; an old result cannot overwrite a new binding or cwd. Discovery is serialized through a single local check slot (`check_limit: 1`). This limit does not consume the eight-Agent scheduling budget.

`canban-workspaces::context::ContextCache` separates canonical repository/worktree path discovery from per-`git_dir` Git status (`GitStatus { branch, dirty, ahead, behind }`, probed in a single `git --no-optional-locks status --porcelain=v2 --branch --untracked-files=normal` invocation that stops parsing entries on the first dirty line and never fetches over the network). Multiple subdirectories inside the same worktree share one `git_dir` status cache entry, and non-repository directories run zero status probes. Matching Project/Card/ManagedWorktree (`worktree_id`) uses the current Project directories, Card workspaces and `Snapshot.worktrees` rather than cached product ownership. A mapping-only edit reuses filesystem facts; title/comment edits do not invalidate associations. Known non-repository results are cached too.

Local filesystem notifications watch shallow discovered directory/Git metadata roots (never recursive working-tree watchers): `HEAD`, `HEAD.lock` and `.git` changes invalidate cached Git status immediately, while `index`, `FETCH_HEAD`, `ORIG_HEAD` and `packed-refs` changes apply a leading-edge 5-second throttle with zero background timers so idle heartbeats remain completely silent. Native file-event acceptance must run outside a restrictive process sandbox; watcher registration errors are logged. Explicit `terminal.context {refresh:true}` resets discovery and works when a watcher is unavailable.

Selecting a Terminal requests on-demand `terminal.context`: the server reuses a fresh result and invalidates cached Git status after 15 seconds locally (catching unstaged working-tree edits on tab switch without recursive file watching) or 30 seconds on remote SSH hosts. This age is checked strictly on demand, never by a background timer. Explicit refresh also works. Ordinary runtime status changes and heartbeat health checks do not refresh Git.

`runtime.observation` exposes non-sensitive diagnostic counters (connection generation, inventory size, cumulative per-cache probe batches, pending context work and check budget) for acceptance. It does not query Core or filesystem state.

## Exit and attention policy

A confirmed exit without a reported outcome marks the running assignment Failed, saves available output and clears the runtime input wait/notification. The Card stays in its stage without an automatic retry or workflow advance. The Board keeps a failed-profile status visible; this is not a Needs you decision. It does not gain a synthetic task-human gate. Completed reports remain completed after exit. Explicit task `blocked` and human Review gates remain independent.

On startup, legacy finished `needs_report` assignments become Failed. Only the exact historical automatically generated missing-outcome gate, bound to that request, is cleared; independent task/review gates remain. Confirmed preparation/launch failures are Failed; unknown launch effects still require recovery. A known undelivered-work handoff still requires a human decision before changing environments.

## Verification

Use `mise run test`, `mise run build`, workspace Clippy and `mise run acceptance-events`. The latter uses an isolated Core and disposable home, two real Shell PTYs and a deterministic interactive Agent. It records RPC method names, verifies no list/get/status scan or extra Git probe across an idle heartbeat, shared-cwd discovery, HEAD changes, cwd changes, a simulated `missed` snapshot, permission-wait exit, saved output, connection loss and reconnect. It does not run a paid provider or inspect credentials.

The existing Overview, scheduling and retained-interaction acceptance checks cover runtime slot release, human gates, cross-Board admission, one-minute idle deadlines, human-input reset, exact Resume and Archive. Provider authentication/model quality and system-wide CPU savings are not established by these fixture checks.

Acceptance evidence from this change (temporary artifacts):

- Local event/cache run: `/private/tmp/cb-events-n7o93wza/evidence.json`; no idle inventory/status/Git query across a heartbeat, HEAD and newly created `.git` invalidation, missed/reconnect, failure and output checks passed.
- Overview: `/private/tmp/cb-overview-iobgsdo3`; scheduling: `/private/tmp/cb-scheduling-xppd2htk`; retained interaction: `/private/tmp/cb-retained-5_w2hro5`. These isolated real-PTY fixtures passed their full checks.
- Native production App: `/private/tmp/cb-events-native-dcvrsdrk/evidence.json`. CUA opened the permission wait, entered `/exit` in libghostty, observed the badge/count clear and the tab disappear, then restarted the UI and opened Failed history and saved `NORMAL_EXIT` output.
- devpc remote event acceptance: `/private/tmp/cb-events-remote-hmz5wsfd/evidence.json`. Remote cwd/branch discovery and explicit refresh, runtime permission/exit, immutable-owner route rename, removal without a false exit and re-add/reconnect passed. Machine-specific isolation/cleanup details are kept in `~/Workspace/Machines/machines/devpc/runtime-events-20261109.md`.

No live user Core or Canban database was changed. Native FSEvents acceptance required an unsandboxed process; the first sandboxed attempt did not deliver filesystem notifications and was not counted as a pass.

## Terminal directory projection (2026-11-16)

`TerminalContext.directory_root` carries the canonical Git checkout root, or the
canonical current directory outside Git. It is resolved in the existing shared
background context cache, never by UI rendering. Shells and Agents consume the
same foreground cwd events. Same-binding last-known contexts survive temporary
observation loss; known external locations do not fall back to launch Projects.
The desktop API version advances with this additive snapshot field.
