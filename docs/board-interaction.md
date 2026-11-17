# Board placement and Card motion

Status: user-approved interaction, implemented 2026-11-10. This replaces the earlier handle-only horizontal stage drag. Verification below distinguishes native checks from layout/model tests.

## Layout and Card surface

The Project picker is the Board header; there is no repeated Board caption above it. Work location and Add Card remain reachable alongside it. Columns retain stage names/counts and their Profile/default tooltip.

The existing window title bar also shows other Projects' compact activity summaries for direct switching. This is Board-only chrome; the dedicated Terminal layout is unchanged. See [Project activity and navigation](quick-open-and-navigation.md#board-title-bar-project-activity-2026-11-10).

A Card's entire surface accepts click/drag. The small Grip is a decorative icon without its own button background or exclusive gesture target. Click opens the Card; native pan recognition uses a movement threshold before claiming a drag, suppressing click activation afterward. Card hover slightly lightens its background through the shared `CARD_HOVER` semantic token. Its ordinary low-contrast border does not change on hover or selection. Selected fill and keyboard focus remain distinct from hover.

### Compact Card tile and stage tabs (updated 2026-11-14, Card #58)

```text
#16  Title wraps beside the number            (3) [>_]
GitHub Issue #12 · open · by author          (GitHub Cards only)
First body line, truncated                    (when present)
Needs review  ⎇ branch          [((●)) ✦ @developer]   (when present)
```

The number shares the title row, and delivery/merge progress, gates/failures, branch and active Agents share one footer, so quiet Cards take two rows. Cards actively merging (`Snapshot.merging_cards`, UI-initiated merge in flight, or an unheld `waiting_delivery` workflow step) render a live `Merging` status chip (`ui-kit::live_status("Merging", Tone::Info)` with `live_dot`) in the footer and Card header until merged, held, or paused. Clicking the Card body opens the Card with its Terminal split collapsed (`card_terminal_open = false`). The top-right Terminal (`[>_]`, **Open Terminal for Card #N**) button reserves its space so hovering never rewraps the title, stays pinned with a highlighted icon (`ACCENT`, or `ATTENTION` when waiting on user input, without a badge dot) when the Card has any live Terminal/Agent (or reveals muted on hover when idle), and opens the Card page with its side-by-side Terminal split expanded rather than jumping away to standalone `Terminals`. Active workflow Agent pills (`@profile`) and ephemeral CLI Agent pills on the Card use `ui-kit::live_dot` (or an attention indicator when waiting on user input) and open the Card page with that exact Agent tab expanded and focused. Columns are capped at 420pt so wide windows keep Card proportions. When the Card pane is open and the Board is narrower than every column at its 220pt minimum, the Board shows one stage at a time with stage tabs (`Backlog 3 | Develop 1 | Review 1`); the shown stage follows the opened Card. Dragging then reorders within that stage; stage moves use the Card stage menu. Gallery: `CANBAN_GALLERY_SCENE=board-cards` and `board-cards-open`.

## Placement preview

```text
Backlog                    Develop
                           +----------------------+
[dragged Card leaves gap]   | Existing Card A      |
                           +----------------------+
                           .----------------------.
                           | insertion placeholder|
                           '----------------------'
                           +----------------------+
                           | Existing Card B      |  moves down
                           +----------------------+
```

The dragged Card floats above scroll clipping and follows the pointer directly through NSH native pan/animation values. It has no delayed follower animation. The target column contains a full-height placeholder before release; peers move to make space. Empty columns accept a placeholder at their start. Leaving the Board/column viewport invalidates the drop; Escape cancels. Source and target lists spring back on cancellation.

The pointer is tested against measured column regions and resting Card midpoints, with a small hysteresis band. Animated peer frames are not used as moving thresholds. Column scroll offsets and horizontal Board scrolling participate in the same coordinate mapping. Near an edge, scrolling advances continuously with elapsed frame time rather than a fixed number of pixels per callback.

## Motion and update ownership

- `ui/board/drag.rs` owns the gesture, measured viewport geometry, lifted overlay, edge scrolling and final placement intent.
- `ui/board/model.rs` computes resting row coordinates (`Layout::resting`) alongside insertion/placeholder/peer target positions without changing saved Cards or dispatching work.
- `ui/board/columns.rs` keeps keyed native row owners positioned at their real layout `top` (`resting_y`), uses `bind_translate_y` only for transient drag displacement (`target_y - resting_y`, reset to `0` when idle), and renders each row through a per-Card `computed` `CardTileModel` with keyed tile subviews so unrelated `Snapshot` updates do not re-reconcile unchanged Cards. Only changed position targets receive a new native spring; new targets interrupt from the current position/velocity.
- `ui/components/card_tile.rs` owns Card content and surface presentation. UI Kit owns shared hover color, controls and NSH adaptation.

Card movement itself uses NSH's native animated graph. A display-frame task reads the current pan values for hit-testing/edge scrolling; it does not rebuild the Board for every pointer pixel or contact AppServer. Placeholder/row targets change only when the insertion slot or measured layout changes. Background snapshots are deferred while the gesture/placement settles, preserving pointer capture and the original revision. They are reconciled afterward. Navigation/disposal ends the local gesture owner without replaying a placement.

Peer movement and release/cancel use a critically damped spring, retaining current velocity on retarget. The lifted overlay stays mounted until the native landing animation completes and the operation receipt is available, rather than disappearing after an estimated timer. Reduced motion sets positions directly. Backend failure returns the preview and shows the operation error; uncertain effects require reconciliation, not automatic replay.

## Durable order and workflow

`card.move` accepts optional `before_card_id`: an active Card in the destination Project/stage, or explicit null to append. Omitting it on a same-stage call retains the old no-op behavior. Drag placement always supplies it. The source Card revision and existing request-key receipts apply.

Same-column movement changes only Board order, increments that Card's revision and records `card.order_changed`. It preserves human gates, queued/running work and completion callbacks. Cross-column movement saves stage/order atomically through the existing move operation, cancels obsolete queued stage work and enqueues the destination workflow once. Preview never changes a stage or starts an Agent.

Cards store optional sparse `board_order` ranks in their existing JSON records. Earlier Cards without a rank retain deterministic number order; no database rewrite is needed. A rank gap normally changes only the moved Card. Rare numeric normalization preserves peer relative order and content revisions, so it is not an additional user reorder. New Cards and ordinary workflow stage transitions append. Archive/Restore retain identity/history; removed/archived Cards are excluded from active drop targets.

API level 4 prevents a new desktop from connecting to an older service that would ignore placement. `mise run run` launches the matched development UI/AppServer. The Gallery supplies a local fixture handler for the same typed `board::Drop` intent; production has no fixture records or fake scheduling branch.

## Verification

Acceptance must cover Card-body versus Grip start, click without movement, cross-column insertion, same-column reorder without dispatch, an empty target, cancellation/outside release, actual pre-drop placeholder/peer movement, long-column and horizontal edge scrolling, background updates, failed/stale placement, retained order after restart, Theme hover and reduced motion. Use production Gallery components and an isolated real-Herdr application; passing layout tests alone is insufficient.

### Executed verification — 2026-11-10

The combined working tree (including the preceding Routing tag removal) passed `mise run build`, `mise run build-gallery`, strict workspace/all-target Clippy, `mise run test` (143 tests), `mise run acceptance` and `mise run acceptance-dev`. Backend regressions cover durable/idempotent order, preserved human gates, cross-stage dispatch exactly once, invalid/stale placement rollback, sparse-rank compaction and appended new Cards. Geometry tests cover variable-height placeholder displacement, empty columns and midpoint hysteresis.

Native application checks used the packaged development binaries in a separately registered acceptance bundle, isolated Canban home and isolated Herdr Core. Deterministic fixture Agents ran through real Herdr PTYs; this was not a new Codex/Claude provider acceptance. Evidence is under `/private/tmp/cb-board-native-bsf2zpmw/` (`before.json`, `final-snapshot.json`, `final-checks.json`, `failed-drop.json`, `stale-write.json`, `server.log`, and the final `evidence.json`). These temporary paths are local reproduction evidence, not permanent artifacts.

- Dragging Card #3 from its body reordered Backlog to #3/#1/#2/#4, with one order event and no new Agent request.
- Dragging #1 inserted it between Develop #5/#6, persisted that order and created exactly one destination-stage request.
- Releasing #2 above the columns cancelled without changing its revision/stage or creating work.
- A concurrent Card #4 edit during its drag caused `revision_conflict`; the preview returned home and no stage request was created.
- Dropping #3 into empty Review succeeded and reached its ordinary report/human Review gate. Final order was Backlog #2/#4, Develop #5/#1/#6, Review #3, with four total requests versus two initially.
- Ordinary Card clicks still opened details, and closing details restored the Board. No redundant Board caption or separate Grip button remained.

The final production-component Gallery used the 72-Card overflow scene at 1200 × 760 logical points. After scrolling Backlog, dragging visible Card #6 from its body inserted it between Develop #25/#26; counts changed from 24/24/24 to 23/25/24 without opening details. Earlier Gallery checks also exercised same-column movement. The final drag trace recorded two layout-target changes; native animation values handled pointer movement.

Automation supplies an atomic drag and cannot hold the pointer while observing intermediate frames. Sustained vertical/horizontal edge-hold scrolling, Escape during a held gesture, reduced-motion system settings and hover across Themes still need manual native checks. Placeholder geometry and motion ownership were checked in source/tests; synthetic drag timings do not establish display frame rate or end-to-end input latency. The isolated app and Gallery were closed after checks, and their test Core was stopped.

### Bottom-Card lift regression — 2026-11-12 (superseded row-offset mechanics in Card #76, 2026-11-15)

*(Historical 2026-11-12 mechanics — superseded below by `top: resting_y` + delta-only `bind_translate_y` in Card #76)*:
NSH `Measurement::page` includes ancestor scrolling but excludes transforms.
Board rows originally had layout `top: 0` and received their resting Y position through
`bind_translate_y`. The lifted overlay had to add the captured resting row offset
once to the measured page Y before deriving both its Board-local origin and
its landing/cancellation displacement. Using the measured page Y alone made
lower Cards jump upward by their row offset (including in scrolled columns).
The row was frozen while lifted; idle layout set its transform directly.

Run `CANBAN_GALLERY_SCENE=board-bottom-drag mise run gallery` for four Cards with
uneven heights. Drag the last Card by its body and grip, first a small distance,
then into an empty column, and release outside the Board to cancel. The grabbed
point must remain under the pointer and the Card must land without an extra
vertical jump. Also use `CANBAN_GALLERY_SCENE=board-overflow mise run gallery`,
scroll to the last Card, and repeat to check that scrolling is not counted twice.

Executed checks: `mise run build`, `mise run test` (257 passed),
`mise run acceptance`, Rust formatting and `git diff --check` passed. The native
`board-bottom-drag` Gallery exercised body-start small movement, a drop into an
empty Develop column, and outside-column cancellation. A mid-gesture capture
showed the lower Card next to its original placeholder, with no upward jump.
The packaged application was also exercised using `mise run acceptance-native`
with a disposable home and isolated Herdr Core. After scrolling to the bottom,
Card #13 moved before #12; snapshot comparison confirmed only #13 changed,
its revision advanced once and the request count stayed at one. A subsequent
small drag of bottom Card #12 retained its revision/order; a mid-gesture capture
confirmed its lifted origin remained at the scrolled row. The native app needed
foreground input for these drags; background events produced no persistent
change. Evidence was saved with Card #20's development report. These checks do
not cover reduced-motion settings, horizontal scrolling or sustained edge holds.

### Asynchronous Review delivery and Board gesture ownership — 2026-11-13 (Card #37)

Live `AppServer` mutations (`card.move`, `card.hold_auto_merge`, `workflow.decide`,
`workflow.deliver`, `review.merge`, and `continue_card`) and runtime ticks wake a
dedicated background delivery worker (`request_deliveries` / `start_delivery_worker`)
instead of running `pump_deliveries()` (`git fetch` + `git merge` + `git push`)
synchronously before returning the RPC response. Dropping a Card with a branch
directly from `Backlog` into `Review` therefore returns its `card.move` receipt in
milliseconds, lands the Card immediately with the live `Merging` status chip
(`Snapshot.merging_cards`), and keeps peer Cards clickable and draggable while
upstream `git push` runs in the background. In-process unit tests constructed via
`AppServer::open` without `start()` retain synchronous `pump_deliveries()` behavior.

`BoardDrag` scopes drag eligibility to in-flight `"card.move"` requests rather than
global `ui.pending`, keeps NSH `pan_enabled` mounted on the lifted row across active
gesture phases (`1` and `2`) so `EventKind::PanEnd` fires on mouse release while
disabling pan on peer rows and during landing (`3`), runs its spawned tasks and
receipt signals under `BoardDrag`'s Board-scoped reactive `Owner`, bounds spring
settlement waits, and aborts an active gesture if the lifted row unmounts.

### Deep-column scroll culling and keyed Card tile reconciliation — 2026-11-15 (Card #76)

NSH's scroll offscreen culling (`Core::sync_culled_in`, `RECLAIM = 3.0` viewports) and
macOS `Sights::shows` evaluate layout `frame.origin` rather than animated `translate_y`.
When a column exceeded ~28 Cards (`scroll_y > 3 * viewport_h`), keeping every row at
layout `top: 0` caused NSH to traverse into row children at `cy ≈ 0..112` and reclaim
pure `View`/`Text` subtrees in Cards #29+, leaving blank Card shells (with only `Canvas`
icons surviving) and causing deferred/fallback row-height overlap or duplicate text
during `Snapshot` updates across unkeyed conditional tile children.

Board rows now bind layout `.top(resting_y)` (`Layout::resting`) so offscreen culling,
AppKit layer visibility, and `Measurement::page` all see true vertical coordinates, while
`bind_translate_y` carries only the active drag displacement `target_y - resting_y`
(`0` at rest; `BoardDrag::start` uses `reference.measure()` directly without re-adding
`original_y`). Each row derives a `computed` `CardTileModel` (`PartialEq`) and keys all
conditional/list subviews in `card_tile` (`header`, `github-meta`, `excerpt`, `tags`,
`footer`, status chips, and Agent pills) so unrelated `Snapshot` mutations skip unchanged
tiles cleanly. Verified with NSH `Core` deep-scroll tests (`deep_review_column_retains_bottom_card_texts_after_scrolling_past_reclaim_threshold`),
`CANBAN_GALLERY_SCENE=board-deep-review` (36 Review Cards), and an isolated packaged
`Canban.app` instance with 33 Review Cards + live `Backlog` mutations and bottom-Card
drag reordering.

### Large Board ownership

Board geometry retains the complete filtered order and measured/estimated heights.
Native row owners cover the viewport and one viewport of overscan, with a minimum
two-row buffer. Scroll events update that range without rebuilding the complete
Card collection. The lifted and keyboard-focused owners are pinned until their
gesture/focus ends; both resting and displaced peer bounds participate in culling.
Height callbacks merge into one reflow per display frame, reusing ordered indices.
Unchanged filtered Card inputs retain geometry during synchronization status updates.

`CANBAN_GALLERY_SCENE=board-large mise run gallery` exercises production components.
Set `CANBAN_GALLERY_CARDS=5000` for scale, or `CANBAN_GALLERY_SNAPSHOT=/absolute/export.json`
for a read-only exported Snapshot. Combining them scales that export. Gallery has no
AppServer or provider writes. `CANBAN_BOARD_TRACE=1` records Board sync/reflow time
and backend synchronization batch lock time. These are model/lock intervals, not
click-to-present latency. The ignored `exported_project_switch_benchmark` test uses
`CANBAN_BOARD_BENCH_SNAPSHOT` and reports Core commit p50/p95 and native Card owner
counts without claiming GPU presentation timing.
