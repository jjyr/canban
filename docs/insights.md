# Insights

Insights opens on **1D**, the current local calendar day. It summarizes retained
workspace activity, not an inferred personal contribution score.

## Periods and comparisons

The header retains All Projects / single Project scope and 1D / 7D / 30D / 90D.
Previous / next day, Today, and an ISO date field select the period's ending date.
Future dates are unavailable. Returning from a Card retains the Insights scope,
range, date, expanded timing section, sample tab and scroll position.

Today runs from local midnight through the current clock time. Its default
baseline is yesterday through the same local clock time; 1D also offers the same
day last week. Historical dates compare whole days. Multi-day periods compare
the preceding equally sized calendar period, truncated to matching progress
when the current period includes today. Boundaries resolve in the display
machine's local timezone, not by subtracting fixed 24-hour durations.

The daily KPI reference uses up to seven preceding dates at the same cutoff,
with the number of available dates shown. Missing baseline coverage displays
insufficient history. A known zero baseline shows an absolute change instead of
an infinite percentage. Quantities do not receive a positive efficiency grade
merely for increasing.

## Layout and metrics

The page order is:

1. Selected dates, comparison and four summary tiles: created Cards, merged
   Cards, contributing Projects and Agent profiles/providers used.
2. For 1D, an hourly cumulative comparison with its own metric selector.
3. Daily created/merged throughput, with a seven-complete-day moving mean.
   The 1D view selects 14 or 30 recent days; longer ranges follow the selected period.
   Today's unfinished bucket has no complete-day moving mean.
4. Project contributions, sortable by merged or created Cards, including merged
   share, unique active Cards, profiles and a seven-day merge sparkline.
5. Agent usage, grouped by Profile or provider, with new starts, unique Cards,
   queue median and trigger origin.
6. A full-width annual tile grid, independently selecting merged Cards, created
   Cards or Agent starts. It always covers the last 365 local dates including
   today, even while inspecting a historical day or a different detail range.
7. Collapsible efficiency details: execution-to-merge and queue-to-start p50/p90,
   baseline values, absolute duration changes, sample sizes, and sample lists.

The annual grid uses one metric at a time; it never adds merge and run counts.
It includes active-day and streak summaries; an unfinished empty today does not
break the streak ending yesterday. Selecting a date switches to 1D and scrolls
the overview into view. Narrow windows retain the annual row as a horizontally
scrollable grid, use two columns of summary tiles and wrap detail rows. Resizing
the annual viewport scrolls to the latest dates.

Created / merged Card lists drill into Cards. Project rows narrow the page scope.
Timing rows switch between merge-cycle and queue samples, longest first, with
20 samples per page and direct Card navigation. No separate activity feed,
run-result summary or current attention inventory is included in efficiency.
Those inventory concepts retain their existing application surfaces.

## Counting and data coverage

- A merge is one unique successfully merged Card, including retained archived
  Cards. Unmerged Archive and an Agent's completed report do not imply delivery.
  Local merges and GitHub PR merges remain separate subcounts.
- The merge date must come from a positive delivery timestamp or an actual
  merge event. Creation and archive times are never substitute merge dates.
  Records without a reliable merge date are disclosed and excluded from dated
  totals and charts.
- A Project is active when a retained Card is created, merged or actually
  executed during the selected interval. Active Cards and participating
  profiles are sets, not sums of scheduling events.
- Agent usage includes executions overlapping the interval, including runs
  started before midnight. New starts belong to their actual launch date.
  Unstarted queued tasks and report-only history without launch evidence are
  excluded from usage. Multiple Agents may touch the same Card; their Card
  counts cannot be added to derive unique output.
- Batch merger execution is counted once for the batch, with all participating
  Cards attached. `MergeBatch.queued_at` and `started_at` are optional durable
  scheduling/launch facts. New launches record them; legacy batches are not
  backfilled from integration/creation time. Missing launch coverage is disclosed.
- A cycle sample is first observed actual execution to merge, including elapsed
  waiting. Queue samples require both enqueue and launch timestamps. Both
  expose the number of valid current/baseline samples. p50 is the median, and
  p90 uses nearest rank. Samples do not estimate human work time or Agent CPU time.
- Provider grouping prefers retained Agent run configuration, falling back to
  current Profile configuration. Unknown providers remain explicit. Configuration
  changes before retained run history can limit historical provider attribution.
- The oldest retained evidence defines the displayed history boundary. It does
  not prove that every historical event is available. Removed Card history may
  be absent from snapshots. Offline host names and last sync times disclose stale
  cached data; their records are not silently represented as fresh observations.

The UI projects the existing complete retained snapshot and reacts to snapshot
changes plus the shared runtime clock. It adds no per-tile queries or polling.
The optional batch timestamps extend the snapshot contract (Desktop API 65 after integration with main),
without changing SQLite schema or rewriting legacy JSON records.

## Verification surfaces

Gallery scenes `insights`, `insights-empty`, `insights-efficiency` and
`insights-short-history` use a fixed clock and production components. The rich
fixture includes a full year, two Projects, multiple Profiles and unmerged
archived Cards. Model tests exercise cutoff comparisons, local midnight,
historical dates, leap-year annual coverage, cross-day execution, duplicate
observations, missing timestamps and percentiles. Native interaction evidence
is recorded separately from model / mounting tests.

[Native and automated acceptance, 2026-11-17](insights-acceptance-20261117.md)
records the exercised builds, fixtures, UI interactions and verification limits.
