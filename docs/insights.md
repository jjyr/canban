# Insights

Insights opens on **1D** (current local calendar day) with a **timeline-first** layout combining personal active dwell time (`My time`), Agent working duration (`Agent working`), Kanban delivery throughput, automation funnel metrics, Project and Profile/Model Agent breakdowns, a 365-day activity heatmap, and always-visible timing percentiles and stage durations.

## Periods, scope, and comparisons

- **Top Chrome**: Project scope (`All Projects` or a single Project) and Range (`1D`, `7D`, `30D`, `90D`).
- **Period Sub-bar**: Previous / next day steppers, `Today`, an ISO `YYYY-MM-DD` date field, baseline selector (`Previous period` or `Last week, same day` in `1D`), inline `Created Cards` / `Resolved Cards` drilldown toggles, and `Clear Project filter`. Future dates are unavailable. Returning from a Card retains scope, range, date, timing drilldown state, and scroll position.
- **Same-Progress Cutoffs**:
  - Today runs from local midnight through the current clock time, comparing against yesterday (or last week's same weekday) through the **same local clock time**.
  - Historical dates compare full calendar days. Multi-day periods (`7D`, `30D`, `90D`) compare the preceding equally sized calendar period, truncated to matching progress when the current period includes today.
  - All boundaries resolve in the display machine's local timezone rather than subtracting fixed 24-hour durations.

## Timeline-first cockpit layout

1. **Summary & 4-KPI Strip**:
   - **Summary sentence**: Highlights period resolved count (`local` / `PR`), `My time`, `Agent working` time, and zero-human resolution rate.
   - **RESOLVED**: Period resolved Cards (`+Δ%` vs baseline), local merge / PR merge / manual resolution breakdown bar, organic created count, net WIP delta, and 7-day daily reference pace.
   - **MERGES (LOCAL / PR)**: Local vs PR merge split, merge queue batches, and clean batch rate (or current WIP when no queue batches exist).
   - **MY TIME**: Active desktop window dwell total (`Δ` vs baseline), proportional `Card / Terminal / Other` bar, and per-lane breakdown.
   - **AGENT WORKING**: Total Agent working time (`Δ` vs baseline), `Completed / Active / Blocked / Failed` run outcome bar, run count, pass rate, active profiles, and human asks.
2. **Hero Activity Timeline**:
   - **In `1D` (Hourly Swimlanes)**: Horizontal 00:00–24:00 swimlanes for `Me · Card`, `Me · Terminal`, `Me · Other`, and active Agent lanes (grouped by `Profile` or `Model` matching the Agents section toggle), plus clickable `Merges` markers. Blocks are color-coded by Project, reveal exact intervals in tooltips, open the underlying Card or Terminal on click, and dim future hours beyond the current clock marker.
   - **In `7D` / `30D` / `90D` (Daily Activity)**: Daily rows comparing stacked `My time` (`Card / Terminal / Other`) against `Agent working` duration and daily resolved count; clicking any day row drills into `1D` for that date.
3. **3-Panel Overview Row**:
   - **Output**: On `1D`, cumulative hourly `Resolved`, `Created`, or `Agent starts` vs baseline day with blank future hours. On `7D`–`90D`, daily `Created / Resolved` bars with a 7-complete-day moving mean; clicking a bar opens `1D` for that date.
   - **Automation funnel**: Period `Created → Agent touched → Reviewed → Merged` funnel, zero-human completion rate (`Δ` vs baseline), human asks, and live Board WIP (`Backlog / Develop / Review`), `Human gated`, `Merging now`, and `Attention` counts.
   - **Projects**: Sortable by `Time`, `Resolved`, `Created`, `WIP`, or `Runs`, showing each Project's timeline color dot, `My time` + `Agent time`, resolved count, and an `Other / unattributed` row when all Projects are in scope. Clicking a row filters Insights to that Project; clicking again clears the filter.
4. **Agents used**:
   - Switchable between **`Profile`** (`@role (provider · model)`) and **`Model`** (`model (providers · N profiles)`), displaying working duration, period runs, outcome bar, resolved `Pass %`, unique Cards touched, `Queue p50` with baseline delta, human asks (`execution.input_required`), and trigger-source shares (`Workflow Auto`, `@mention Comment`, `Manual / Interactive`).
5. **365-Day Yearly Activity Heatmap**:
   - Full-width calendar heatmap switchable across `Resolved`, `My time`, and `Agent time` over the last 365 local dates including today, lazily fetching daily activity buckets on demand and reporting active days and streaks. Selecting a tile opens `1D` for that date.
6. **Timings (Always Expanded)**:
   - `Execution → resolution` and `Queue → start` percentile table (`p50`, `p90`, `Baseline p50`, `Δ`, `Samples`) with inline paginated slowest-first `[Inspect]` Card drilldown, paired with stage & merge-queue averages (`Develop`, `Review`, `Merge Queue`, `@mention Reply`, clean batch rate, average batch size, and conflict resolution duration).
7. **Intake & Coverage Footer**:
   - Single-row intake breakdown (`Local manual`, `GitHub PR`, `GitHub Issue`, `Initial sync`) with an inline `Exclude initial sync` / `Include initial sync` toggle, plus retained history start, undated exclusions, and offline-host notices.

## Time and Agent activity collection

- **Personal dwell (`My time`)**: `crates/ui/src/activity.rs` tracks page, Card, and Terminal focus transitions via `DwellMachine` (`Card`, `Terminal`, `Other` lanes) and checks macOS `CGEventSourceSecondsSinceLastEventType` on the existing 60-second `utility_clock` interval (zero extra background timers, zero keystroke/pointer hooks, and zero cross-application focus sampling). Activity enforces a 300-second idle cutoff (with 60-second clock grace), drops segments shorter than 5 seconds, checkpoints every 5 minutes, caps in-memory pending segments at 500, and flushes closed/checkpointed segments to the local host via `activity.record`.
- **Agent working duration (`Agent working`)**: `crates/app-server/src/observation.rs` & `activity.rs` derive Agent work directly from Herdr `Inventory` state transitions (`status.state == "working"` and state != `"finished"`, minimum 2 seconds). Completed intervals flush during runtime `tick()` and server shutdown; live open `working` intervals are merged on query.
- **Storage, RPCs, and multi-host merging**: Segments persist in SQLite schema `v21` table `activity_segments` without foreign keys (so deleted Cards/Projects keep historical durations) and prune automatically after 400 days alongside event pruning. `activity.record` routes strictly to the local host (`canban-ui` also falls back gracefully on remote/deleted project IDs), while `insights.activity` (`<= 8` days, boundary-clipped + live open intervals) and `insights.activity_buckets` (`<= 400` boundaries) query all connected hosts and merge local dwell with per-host Agent activity. UI ↔ AppServer compatibility uses `DESKTOP_API = 76`.
- **Intentional omissions & scope**: Commit counts and token/cost usage statistics are intentionally omitted (Herdr exposes only process/agent status transitions without usage or token accounting). Dwell records active Canban window focus (`Me · Card`, `Me · Terminal`, `Me · Other`) and does not track time spent in external tools or distinguish finer-grained `agy` screen-refinement sub-states beyond Herdr's `working` state.

## Verification surfaces

Gallery scenes `insights`, `insights-weekly` (`7D` multi-day view), `insights-agents-model` (`Model` grouping view), `insights-empty`, and `insights-short-history` use a fixed clock (`INSIGHTS_NOW`) and production components with synthetic dwell/agent activity (`fixtures::insights_activity`), bulk initial-sync fixtures, multiple Profiles/Models, and two Projects. Unit, storage, AppServer, dwell, and UI mount tests cover schema v21 migrations, `Inventory` working transitions, `activity.record` / `insights.activity` / `insights.activity_buckets` RPCs, `DwellMachine` idle cutoffs, model aggregation, timeline lanes, and interactive range/group/metric switching.
