# Insights

Insights opens on **1D** (current local calendar day) with **Daily** velocity cadence, summarizing retained workspace throughput, Kanban pipeline health, Agent execution quality, automation leverage, and Day-over-Day / Week-over-Week efficiency trajectories.

## Periods, cadence, and comparisons

- **Top Chrome**: Project scope (`All Projects` or a single Project), Range (`1D`, `7D`, `30D`, `90D`), and Trend Cadence (`Daily` vs `Weekly`).
- **Period Sub-bar**: Previous / next day steppers, `Today`, an ISO `YYYY-MM-DD` date field, baseline selector (`Previous period` or `Last week, same day` in `1D`), `Organic intake (excl. sync)` toggle, and inline Created / Resolved Card drilldown toggles. Future dates are unavailable. Returning from a Card retains scope, range, cadence, date, expanded timing section, sample tab, and scroll position.
- **Same-Progress Cutoffs**:
  - Today runs from local midnight through the current clock time, comparing against yesterday (or last week's same weekday) through the **same local clock time**.
  - Historical dates compare full calendar days. Multi-day periods compare the preceding equally sized calendar period, truncated to matching progress when the current period includes today.
  - **Week-over-Week (WoW)** compares the current ISO week (`Monday 00:00` through the current weekday/clock cutoff) against the previous ISO week at the exact same weekday/clock cutoff, and projects full-week resolution pace when the current week is in progress.
  - All boundaries resolve in the display machine's local timezone rather than subtracting fixed 24-hour durations.

## Multi-dimensional cockpit layout

1. **Four-Dimensional KPI Strip** (each tile pairs a headline metric, comparison badge, proportional breakdown bar, and secondary context):
   - **Resolution Throughput**: Resolved Cards (`+Δ%` vs baseline), Local merges, PR merges, and manual or Agent resolutions, Organic Created count (with muted initial-sync count disclosed), and 7-day daily reference pace.
   - **Pipeline & WIP Health**: Active unresolved Board WIP (`Backlog / Develop / Review` bar), Human-gated count or Net WIP delta, active Projects, and period merge-conflict batches.
   - **Agent Execution Quality**: Period Agent starts, resolved pass rate (`Completed %`), `Completed / Active / Blocked / Failed` bar, active Profiles, Providers, and overlapping cross-midnight runs.
   - **Efficiency & Automation**: Execution-to-resolution `p50` (`Δ` vs baseline), Queue-to-start `p50`, Workflow Auto trigger share, Human asks per resolved Card, and Scheduled Task / Workflow run completions.
2. **Velocity & Personal Efficiency Trajectory (2-column row)**:
   - **Left — Output Velocity**: Switchable between **Daily (`14D` / `30D` or selected range)** with a 7-complete-day moving mean and **Weekly (`8W` / `12W` ISO weeks)** with a 4-complete-week moving mean and outlined full-week projection cap. Clicking a day bar drills into `1D`; clicking a week bar drills into `7D` ending on that week.
   - **Right — Personal & Agent Efficiency Trajectory**: Same-progress WoW summary card (`Resolved`, `Cycle p50`, `Agent Pass %`, `Human asks / resolution`) above a 6-week or 7-day trajectory table pairing output rate, proportional `Cycle p50` bar, pass rate, and human interventions.
3. **Hourly Pace & Intake / Pipeline Funnel**:
   - In `1D`, **Hourly comparison** plots cumulative `Resolved`, `Created`, or `Agent starts` against the baseline day with `nice_ceil_tick` Y-axis scaling and blank future hours, paired beside **Intake sources & pipeline gates**.
   - **Intake sources & pipeline gates** breaks down period Card creation into `Local manual`, `GitHub PR`, `GitHub Issue`, and `Initial sync`, alongside live Board stage counts, `Merging now`, `Human gates`, `Attention cards`, and `Merge batches / conflicts`.
4. **Project & Agent Multi-Dimensional Matrices (2-column row)**:
   - **Project contributions**: Sortable by `Resolved`, `Created`, `WIP`, or `Runs`, showing `+Created / Resolved (share)`, live `WIP (B/D/R)`, `Agent runs`, and a 7-day resolution sparkline. Clicking a Project scopes Insights; clicking the active Project again clears the filter.
   - **Agents used**: Groupable by `Profile` (`@role (provider)`) or `Provider`, showing period starts, unique Cards touched, resolved `Pass %`, `Human asks` (`execution.input_required`), and per-`(profile, provider)` `Queue p50`, plus trigger-origin shares.
5. **365-Day Yearly Activity Grid**:
   - Full-width calendar heatmap independently selecting `Resolved`, `Created`, or `Agent starts` across the last 365 local dates including today, with active-day and current/longest streak summaries (an unfinished empty today does not break yesterday's streak).
6. **Collapsible Efficiency & Timing Percentiles**:
   - `Execution → resolution` and `Queue → start` `p50` / `p90` table with baseline values, absolute duration changes, sample counts, and paginated slowest-first Card drilldown lists.

## Counting, organic intake, and data coverage

- **Organic Intake vs. Bulk Initial Sync**: Cards created with `card.created` `initial_sync: true` (automatic intake when first connecting an existing GitHub repository) are classified as `InitialSync` and excluded from organic `Created` velocity by default so historical issue imports do not flatten daily/weekly charts; toggling `All intake (incl. sync)` includes them.
- **Resolutions** count each resolved Card once, including retained archived Cards. A merge or an explicit user or Agent resolve qualifies. Insights uses the Card resolution timestamp, the delivery timestamp, or a retained resolution or merge event. The footer discloses Cards that lack a reliable timestamp and are excluded from dated totals.
- **Agent Usage & Per-Provider Queue Wait**: Reconstructed runs deduplicate `queued` / `started` / `continued` / `reported` events with retained `TerminalEntry` and `MergeBatch` facts. Provider attribution prefers the run's retained terminal profile preset/command before falling back to current Profile configuration, and `Queue p50` is keyed by `(profile_id, provider)` so provider switches never cross-contaminate medians.
- **Indexed Event Projection**: Live UI rendering reads pre-bucketed `SnapshotIndex.events_by_card` positions rather than scanning cardless repository-observation events on each clock/snapshot tick.

## Verification surfaces

Gallery scenes `insights`, `insights-weekly`, `insights-empty`, `insights-efficiency`, and `insights-short-history` use a fixed clock (`INSIGHTS_NOW`) and production components, including bulk initial-sync fixtures, multi-week cycle trends, two Projects, multiple Profiles, and unmerged archived Cards. Model and UI tests verify organic vs. initial-sync filtering, per-provider queue medians, same-progress WoW and DoD comparisons, DST/timezone boundaries, leap-year 365-day coverage, and interactive cadence/range switching.
