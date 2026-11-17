# Large GitHub Board performance acceptance — 2026-11-18

Card #114, branch `canban-0afd/114`, based on `7086f3bb68baba4510f6b111af277b78872325dd`.
The implementation addresses repeated UI filtering/layout and Store reconciliation
work. No RPC, Snapshot, event or workflow schema changes were required; desktop API
remains 65. No prompt or role instruction was changed.

## Implemented paths

- Board filters use the existing source index and skip source lookup for unrestricted
  filters. Active GitHub identity reads borrow the Snapshot instead of cloning its
  complete global history for each Card.
- Snapshot status indexing computes Merge Queue membership once per Project. The
  prior per-Card lookup rebuilt a full Card map repeatedly, including when no Card
  belonged to the queue. FIFO position, duplicate-run precedence, pause and hold
  semantics have regression coverage.
- Board geometry retains ordered indices and variable measured/estimated heights.
  Native owners cover viewport plus overscan, with lifted/focused rows pinned.
  Native height callbacks coalesce once per display frame. Geometry and drag reads
  borrow Card collections/maps. Equal filtered Cards preserve rows and geometry
  through status-only refreshes.
- Issue discovery indexes tracked/removed identities and cached revisions. Each
  reconciliation transaction builds source/delivery/append-order indexes once;
  event relation indexing is also shared by the batch. Creation-event updates use
  the Project/Card scope index. Workers commit at most 64 items before releasing
  Store, rechecking account/source generations and local removals next time.
  Partial imports remain on a later failure, but last-success advances only after
  the complete sync succeeds. Provider cache reads occur outside Store.

## Core switching measurements

macOS, pinned mise Rust beta toolchain, Debug build, production Board components
through NSH Core and `ApproxTextEngine`, 1200 × 760 logical points. The read-only
export contained 236 Cards and 10,157 global events; Fiber had 100 active Cards.
The 1,000/5,000 scenarios scale Fiber ownership/source metadata while preserving
other Projects and global history. Each row contains 12 empty-to-Fiber switches;
initial Core setup is reported separately. Filters were unrestricted.

| Fiber Cards | Initial Core setup | Switch p50 | Switch p95 | Maximum native Card owners |
| --- | ---: | ---: | ---: | ---: |
| 100 | 321 ms | 85 ms | 109 ms | 12 |
| 1,000 | 451 ms | 85 ms | 92 ms | 11 |
| 5,000 | 1,087 ms | 130 ms | 206 ms | 11 |

These are model/layout commit timings, including Core event dispatch and layout
listeners. They do not measure GPU presentation, click-to-first-pixel latency,
or user input latency during a live GitHub request. They are current-build
measurements, not a controlled release-to-release speedup claim.

Reproduce with `CANBAN_BOARD_BENCH_SNAPSHOT=/absolute/export.json mise run test --
--run-ignored only -E 'test(exported_project_switch_benchmark)' --nocapture` on one
shell line. Logs are `/private/tmp/canban-114-bench-{100,1000,5000}.log`.

## Backend and native checks

- `mise run check`: formatting, workspace/all-target check and Clippy with warnings
  denied passed. `mise run test`: 612 passed, two intentionally skipped. After final
  UI borrowing/queue precedence changes, all 178 UI tests passed, one benchmark
  intentionally skipped. The three opt-in benchmark runs passed separately.
- `mise run build`, `mise run acceptance` and GitHub workspace acceptance passed.
  The GitHub fixture now supports paginated 130-source input. Three worker batches,
  repeat synchronization, numbering, source deduplication, removal suppression,
  stale-generation rejection and unchanged local instructions/history are covered
  by integration/unit checks. An initial scale-fixture pagination error was fixed
  before rerunning the complete GitHub acceptance.
- During the 130-source sync, 96 concurrent `card.get` CLI reads had p50 17 ms,
  p95 64 ms and maximum 82 ms (including CLI startup/RPC overhead). Maximum observed
  Store transaction lock time was 66 ms; the final large import's 64/64/2 batches
  held it for approximately 41/35/11 ms. Evidence:
  `/private/tmp/canban-114-github-concurrent.log` and
  `/private/tmp/cb-github-workspace-9og3lka1/server.log`.
- Native production-component Gallery used the read-only Fiber export, then a
  5,000-Card derivative. Verified initial rendering, scrolling beyond several
  screens, Local filtering/Reset and moving a scrolled Card into an empty column.
  The scaled move changed Backlog/Develop counts from 5000/0 to 4999/1. Evidence:
  `/private/tmp/canban-114-{fiber,large}-{native,scrolled,drop}.png` and Gallery logs.
- The packaged `.local/Canban.app` supplied an isolated acceptance bundle/home/Core
  under `/private/tmp/cb-native-x0qq_fty`. Verified Card open/close, empty/nonempty
  Project switching and retained same-column ordering. Native drag changed only
  Card #3 from revision 1 to 2, emitted one `card.order_changed` event and retained
  one Agent terminal. Before/after snapshots and screenshots are
  `/private/tmp/canban-114-app-*.{json,png}`. The disposable UI/backend/Core and
  Gallery were stopped afterward. The live Fiber Cards were never changed.

Native screenshots prove the exercised interactions; they do not establish frame
rate, sustained edge-hold scrolling, horizontal dragging, a full keyboard/assistive
technology matrix, or display-latency p95 under live synchronization. Large-Board
geometry remains proportional to Card count even though native owners are bounded.
Temporary evidence paths are local artifacts, not permanent bundled fixtures.
