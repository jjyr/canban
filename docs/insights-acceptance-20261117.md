# Insights acceptance — 2026-11-17

Scope: Card #107 on `canban-0afd/107`, based on
`70594805679da068b5f7d7ba01eccaf9f14d824c`. The Card completion report identifies
the final implementation commit. Desktop API is 63; SQLite schema is unchanged.

## Automated verification

- `RUSTUP_AUTO_INSTALL=0 bu exec -- mise run test`: 584 tests passed; one ignored
  test skipped. Includes the real batch-launch timestamp assertions.
- Final UI scope, after the layout/live-clock refinements:
  `RUSTUP_AUTO_INSTALL=0 bu exec -- mise run test -- -E 'package(canban-ui)'`:
  167 tests passed. The 12 Insights tests include calendar midnight, timezone
  differences, 23/25-hour DST days, matching-clock comparisons, historical full
  days, 14/30-day trends, leap-year annual coverage, cross-day Agent usage,
  duplicate observations, missing timestamps and production UI expansion/ranges.
- `RUSTUP_AUTO_INSTALL=0 bu exec -- mise run build`: packaged and codesign-verified
  `.local/Canban.app`. The final build used Bullet hotpatching. Every `__TEXT`
  section of the built UI, including executable code, was compared with the
  packaged executable and matched; code signing changes file metadata/hashes.
- `RUSTUP_AUTO_INSTALL=0 bu exec -- mise run acceptance`: passed over its own
  disposable home and Core, including Develop/Review, reports, mention/blocking,
  Archive/Restore, restart and cross-Project scheduling.
- `scripts/acceptance-insights.py`: passed with an isolated Core and deterministic
  Python Agent over a real PTY. Asserted two Projects, five retained Cards and
  one delivered Card, with actual execution and merge facts. The `--native`
  mode also launched a uniquely identified copy of the packaged production App.
- `git diff --check` and Python fixture syntax validation passed.

## Native interaction

The production Gallery was launched using `mise run gallery` and the fixed-clock
`insights-efficiency` scene. Cua Driver operated only the Gallery and disposable
acceptance App windows.

Observed in Gallery:

- 1D defaults, 14/30-day recent-trend switching and independent annual scope.
- Full-width annual tiles above the expanded efficiency panel.
- p50/p90 cycle/queue comparisons, sample counts and queue-sample switching.
- Readable month headings and two-column KPIs at an 800-point window width.
- Narrow annual scrolling positions the recent months at the right edge after
  resizing; the grid remains a standalone row.
- Selecting the November 16 tile changed the date to a complete 1D period and
  returned the overview to the top.
- No run-results, needs-attention inventory or separate activity-feed section
  appears in efficiency.

Observed in the production App fixture:

- All Projects showed five created Cards, one local merge, two contributing
  Projects and one used Agent Profile, matching the source fixture.
- Previous-day selection and entering/applying a date updated the period.
- The merged-Card list opened the actual delivered Card. Global Back returned to
  Insights with its selected scope and Card list retained.

Native fixture bases included `/private/tmp/cb-insights-jfq2ab2v`; the final
headless fact run used `/private/tmp/cb-insights-0qyl0ag3`. The final narrow
Gallery capture is retained with the Card Agent run artifacts as
`insights-final-narrow.png`. Disposable apps and their Core instances were
stopped after verification.

## Boundaries

This exercised real AppServer/Core/PTYS with deterministic Agents, not paid
Codex/Claude model execution. Empty/missing-history and DST cases were verified
by model tests; the rich Gallery scene supplied the native year-long dataset.
Historical provider attribution can fall back to current Profile configuration.
Legacy batches without exact launch time, merges without a trustworthy date,
removed history and stale offline-host caches remain explicit coverage limits;
no historical timestamps or human work estimates are synthesized.


## Merge integration verification — 2026-11-17

Card #107 was rebased onto `c0470ad4730da82e1e2f51e854d548103976b660`
with implementation commit `b4b520e`. The updater conflict advances Desktop API
from main's 64 to 65, retaining the Insights batch scheduling/launch fields
alongside main's state synchronization contract. The mise conflict retains
`build-profiling`, `profile` and `acceptance-insights` together. SQLite schema
remains unchanged; the API 63 statement above describes the original build.

Fresh verification after rebase:

- Standard `mise run build` passed and signed `.local/Canban.app`.
- Standard `mise run test`: **603 passed, one configured skip**, including
  Insights, state synchronization, Timeline indexing and workflow regressions.
- `mise run test-package`: all **eight** packaging/signing tests passed.
- `mise run acceptance`: passed with disposable home/Core at
  `/private/tmp/cb-w3jezi4x/home`, including restart and three-Board scheduling.
- `mise exec -- python3 scripts/acceptance-insights.py --native`: source-fact
  assertions passed with disposable home/Core at
  `/private/tmp/cb-insights-wma2xw06`. The packaged production App displayed
  five created Cards, one local merge, two contributing Projects and one used
  Agent Profile. Opening the merged Card list reached the delivered Card;
  global Back retained the list and selected Today scope.
- `CANBAN_GALLERY_SCENE=insights-efficiency mise run gallery`: the production
  components displayed the independent annual row above expanded efficiency,
  p50/p90 comparisons and explicit current/baseline sample sizes. Switching
  to Queue samples changed the list from one cycle sample to two queue samples.
  No separate activity feed or run-results/attention blocks appeared there.
- `git diff main --check` passed. Build/test commands used the configured
  toolchain directly; this integration did not require Bullet artifact staging.

Logs and `gallery-efficiency.png` are retained under the Card's
`agents/merger` run directory. Only the disposable fixture App and Gallery were
operated; their UI processes, backend and isolated Core were stopped afterward.
The earlier native acceptance remains historical evidence for the broader
interaction matrix; this merge pass did not repeat narrow-window/DST interactions
or perform a new Instruments measurement. Deterministic Agents exercised real
PTYs; no paid provider execution was used.
