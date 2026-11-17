# Memory profiling and state synchronization

## Implemented boundaries

Canban exposes a small `app.watch` invalidation cursor and conditional
`app.state {after: cursor}`. Local UI subscription waits on the cursor instead of
reading and retaining a full JSON snapshot every 750 ms. A change invalidates the
UI, which fetches state once. Remote supervision makes conditional state requests
and keeps only cursor/GitHub metadata beside its typed offline cache.

The cursor includes persistent server identity, a fresh per-boot instance ID, SQLite
connection total changes, an explicit
in-memory invalidation generation, and runtime observation freshness. It is not a
durable event-journal offset. Rolled-back writes may conservatively invalidate.
The cursor is captured before loading state, so concurrent commits may require an
extra refresh but cannot be acknowledged by an older snapshot. A bounded wait
rechecks every 250 ms without loading histories or holding database locks; this
also covers internal commits that do not emit a notification. Waits stop within
15 seconds and service restart invalidates the old cursor while preserving server identity. Heartbeats
can still refresh observation freshness; this change is not a per-entity delta
protocol.

Timeline pages use `(card_id, number)` SQL selection and limit before JSON
parsing. Active Workflow revisions are selected in SQL. Delivery and remote
health workers use admission inputs instead of full history. UI global event
indexes contain positions and latest sequences; only the selected Card's Timeline
materializes event bodies. Pair indexes with their source Snapshot generation.

Full state still contains history needed by reminders, unread positions, Overview,
report deduplication and archival navigation. Moving this to compact summaries
and lazy history requires preserving those contracts; it is a separate next
step. No history is truncated. The observed footprint is not proof of a leak.

## Build and capture

Use Bullet around Rust tasks as required by the development conventions:

```sh
RUSTUP_AUTO_INSTALL=0 bu exec -- mise run build-profiling
```

The `profiling` Cargo profile inherits release optimization and retains debug
symbols. Packaging uses the dedicated profile artifacts in `.local/Canban.app`;
it does not install the App. On macOS, only this profiling package is signed
with `com.apple.security.get-task-allow` so Instruments can attach; ordinary and
release packages retain their existing signing configuration. For interactive development use the branch-scoped
instance manager; isolated acceptance must also use an isolated Herdr Core.
Preserve this binary and its `target/profiling` symbol artifacts with the trace.
Record the build before optimization and rerun the same scenario afterward.

Set `CANBAN_PROFILE=1` **before starting both UI and AppServer** to enable
`com.canban.profiler` / `PointsOfInterest` signposts. Attaching Instruments does not enable
signposts in an already-running process. Labels include `state.load_db`,
`state.encode`, `rpc.encode`, `rpc.request`, `rpc.decode`, `ui.apply`, `index.build`,
`timeline.load`, `terminal.attach` and `terminal.detach`. Intervals report known
counts/response bytes without scanning the heap or logging user contents.
Without opt-in, no native signposts or per-span string allocations occur.

Choose the actual PID (UI and backend are independent processes):

```sh
ps -axo pid,rss,comm | rg 'canban-ui|Contents/MacOS/canban$'
mise run profile -- --list-templates
mise run profile -- --pid PID --template Allocations --seconds 30
```

`profile` attaches only to an explicitly selected Canban process. It does not
start, terminate or reconfigure the application. A fresh directory contains
`capture.trace`, `record.log`, before/after `vmmap` summaries and metadata with target build,
checkout commit, dirty state, selected PID and capture exit status and target binary UUID. macOS may
require Developer Tools authorization for attachment; failed capture remains a
failed command with diagnostics retained. Trace files can contain process data;
keep them outside version control.

In Instruments, combine Allocations, VM Tracker, Time Profiler and Points of
Interest (or an installed custom template) to correlate signposts with memory
regions and allocation stacks. Use the template listing rather than assuming a
specific template is installed. Attach captures subsequent allocations; to obtain
allocation stacks from process startup, launch the isolated executable through
Instruments with the same home/Core environment and `CANBAN_PROFILE=1`.

## Repeatable comparison

1. Start an isolated instance with a representative fixture and record idle state.
2. Open and leave the same Card repeatedly, returning to the same view.
3. Switch through the same Terminal set; repeat with sustained output.
4. Mark allocation generations between phases and allow settling time.
5. Compare live bytes, persistent allocation stacks, allocation volume, footprint
   peak, malloc fragmentation, CPU and state request frequency separately.

A fixed warm workload should plateau. Stable live bytes with a larger footprint
suggest allocator retention/fragmentation or native resources; growing persistent
allocations need ownership/lifecycle investigation. A passing build or a smaller
single RSS sample is not end-to-end memory acceptance.

## Acceptance: 2026-11-17 (Card #110)

The development worktree was based on `fcaee75064684eb539f0135621899ab3f51d9411`.
Verification used macOS arm64, desktop API 63 and SQLite schema 20, with an
isolated Canban home and isolated Herdr Core; the installed user instance was
not restarted or upgraded.

- `mise run test`: 590 passed, one skipped. A final affected backend/UI rerun
  passed 354 tests after the routing/helper refinements.
- `mise run build` packaged the native application. `mise run acceptance`
  passed real-PTY development/review, blocked mention, report archive/restore,
  restart and global admission-budget scenarios.
- `mise run test-package`: eight passed, including profiling artifact selection,
  debug entitlements on UI/backend/updater, and removal of those entitlements
  when repackaging ordinary artifacts. Existing Linux fixture coverage passed.
- Native background interaction opened a Card containing 30 replies, observed
  an externally committed 31st reply, closed the details, reopened the Card and
  verified existing history remained visible.
- An isolated 31-event RPC fixture returned approximately 30,423 JSON bytes for
  full state versus 175 bytes for an unchanged response. SQL pages of seven
  events recovered all 31 entries without gaps or duplicates. This is a wire
  comparison, not an RSS reduction claim.
- UI Allocations (12 seconds) and backend Time Profiler (eight seconds) completed
  successfully on a debuggable copy of the verified debug bundle. Allocation
  track metadata and the trace container were inspected; emitted intervals for
  RPC decode, UI apply and index construction were also confirmed in unified
  logging. A first attachment without debug entitlements failed and is retained
  as failed evidence, not counted as acceptance.

Evidence is under `/private/tmp/canban-memory-development`, including capture
logs, metadata with executable UUIDs, traces, vmmap summaries and native action
results. Traces and exports may include target environment/process data and stay
outside the repository. The full optimized UI `profiling` build and a long-running
matched-workload memory plateau comparison were not performed in this run.
Full-history DTOs, selected-Card history materialization and remote typed snapshot
copies remain potential next optimizations; preserving reminder/history semantics
is required before removing those inputs.

Final verification limit: after the successful captures, signposts were moved
from the custom `memory` category to standard `PointsOfInterest` so built-in
Instruments templates include them. Clang syntax checking passed. The final App
rebuild was blocked before compilation because the installed Bullet CLI changed
to an unavailable v0.2.0 prebuilt bootstrap and returned HTTP 404; the previous
local stage1 compiler was no longer present. This is recorded in bullet Card #8
(distinct from Card #7's disappearing in-flight compiler inputs). No native Cargo
fallback or toolchain installation was performed. Final App rebuild/capture of
this category refinement remains pending toolchain recovery or explicit native
Cargo authorization.
