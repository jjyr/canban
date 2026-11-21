# Insights

Insights opens on **1D** (current local calendar day) with a **timeline-first** layout combining personal active dwell time (`My time`), Agent working duration (`Agent working`), Kanban delivery throughput, automation funnel metrics, Project and Profile/Model Agent breakdowns, a 365-day activity heatmap, and always-visible delivery and queue timing percentiles.

## Periods, scope, and comparisons

- **Top Chrome**: Date navigation (`◀ <date> ▶ Today`, with clicking the formatted date opening a lightweight `YYYY-MM-DD` date popover), range selector (`1D`, `7D`, `30D`, `90D`), Project scope (`All Projects ▾`), and a fixed `16x16` loading spinner slot.
- **Comparison Sub-line**: Lightweight right-aligned comparison selector (`Compare: [Yesterday so far] [Last week]` in `1D`; `vs prior 7d/30d/90d` in multi-day) plus optional partial-history or active-filter notices. Future dates are unavailable. Returning from a Card retains scope, range, date, timing drilldown state, and scroll position.
- **Same-Progress Cutoffs**:
  - Today runs from local midnight through the current clock time, comparing against yesterday (or last week's same weekday) through the **same local clock time**.
  - Historical dates compare full calendar days. Multi-day periods (`7D`, `30D`, `90D`) compare the preceding equally sized calendar period, truncated to matching progress when the current period includes today.
  - All boundaries resolve in the display machine's local timezone rather than subtracting fixed 24-hour durations.

## Async loading & zero-jitter rendering

- **Coordinated Background Loader**: A single `refresh_data` pipeline fetches `insights.events` (when scope or snapshot generation changes), period + baseline `insights.activity` / `insights.activity_buckets`, and 365-day `annual_buckets` (on demand for `My time` / `Agent time`) in one background task fenced by `request_seq` and committed in a single reactive `batch`.
- **Stale-While-Revalidate & Fixed-Height Skeletons**: Initial cold loads show fixed-height section skeletons matching the exact dashboard footprint. Subsequent range, date, or scope switches keep the last loaded dataset visible at `0.72` opacity while spinning the fixed-size chrome indicator, avoiding `0s` flashes or layout shifts. Switching the heatmap to `My time` or `Agent time` keeps the previous `Resolved` view until annual buckets finish loading while spinning a `14x14` inline slot.

## Timeline-first cockpit layout

1. **Summary & Airy 4-KPI Strip**:
   - **Summary sentence**: Highlights period resolved count (`local` / `PR`), `My time`, `Agent working` time, and zero-human resolution rate.
   - **Resolved**: Total resolved Cards (`▲/▼ Δ`), clickable to toggle the resolved Cards drilldown; hover tooltip carries the local/PR/manual split, organic intake, net WIP delta, and 7D pace.
   - **Merges (Local / PR)**: Total merges and `(local/PR)` split (`▲/▼ Δ` against baseline `prev_local_merges + prev_pr_merges`), with merge queue batch details in tooltip.
   - **My time**: Active desktop window dwell total (`▲/▼ Δ%` vs baseline), with `Card / Terminal / Other` split in tooltip.
   - **Agent working**: Total Agent working time (`▲/▼ Δ`), with run outcomes, pass rate, active profiles, and human asks in tooltip.
2. **Timeline**:
   - **In `1D` (Hourly Swimlanes)**: Grouped single-line rows for `Me` (`Card`, `Terminal`, `Other`), `Agent` (`@profile` or `Model`), and clickable `Merges` (`●`) markers over an adaptive active-hour window (`TimelineData::visible_hour_window`) with a `[24h]` toggle pill. Tracks use proportional `flex_grow` layout without layout feedback loops.
   - **In `7D` / `30D` / `90D` (Daily Activity)**: Daily rows comparing stacked `My time` (`Card / Terminal / Other`) against `Agent working` duration and daily resolved count; clicking any day row drills into `1D` for that date.
3. **3-Panel Overview Row**:
   - **Output (cumulative)**: On `1D`, solid current vs dashed muted baseline cumulative curves with bottom `[Resolved] [Created] [Agent starts]` tabs. On `7D`–`90D`, daily `Created / Resolved` bars with a 7-complete-day moving mean; clicking a bar opens `1D` for that date.
   - **Automation**: Period `Created → Agent → Review → Merged` funnel, `Human asks: N · X.X/card`, and `Zero-human completion: Y% ▲Zpt`.
   - **Projects**: Compact 2-line rows (`Me` duration + bar, indented `Agent` duration + resolved count), an `Other` row when all Projects are in scope, and a bottom `Sort: Time ▾` popover (`Time`, `Merged`, `Created`, `WIP`, `Runs`). Clicking a row toggles that Project filter.
4. **Agents**:
   - Switchable via `Group: [Profile] [Model]`, displaying a single-row aligned table (`Profile`/`Model`, `Runs`, `Working` bar + duration, `Pass`, `Asks`, `Queue p50` + delta) with run outcome and card details in row tooltips.
5. **Past year (365-Day Heatmap)**:
   - Compact `11px` tile grid (`2.5px` gap) switchable across `Resolved`, `My time`, and `Agent time`, with right-aligned `Streak Xd · Longest Yd` in the header and `Less … More` legend below. Selecting any tile opens `1D` for that date.
6. **Timings (Always Expanded)**:
   - Clean 2-row aligned percentile table for `Execution → resolution` and `Queue → start` (`p50`, `p90`, `vs Baseline`, `Samples`, `[Inspect ▸]`) with an inline paginated slowest-first Card drilldown.
7. **Intake Footer**:
   - Single inline row (`Intake  Local X · GitHub PR Y · Issue Z` and `(Initial sync N excluded ☑)` toggle).

## Time and Agent activity collection

- **Personal dwell (`My time`)**: `crates/ui/src/activity.rs` tracks page, Card, and Terminal focus transitions via `DwellMachine` (`Card`, `Terminal`, `Other` lanes) and checks macOS `CGEventSourceSecondsSinceLastEventType` on the existing 60-second `utility_clock` interval (zero extra background timers, zero keystroke/pointer hooks, and zero cross-application focus sampling). Activity enforces a 300-second idle cutoff (with 60-second clock grace), drops segments shorter than 5 seconds, checkpoints every 5 minutes, caps in-memory pending segments at 500, and flushes closed/checkpointed segments asynchronously to the local host via `activity.record`, incrementing `ui.activity_flush_seq` on successful flushes so Insights updates automatically.
- **Agent working duration (`Agent working`)**: `crates/app-server/src/observation.rs` & `activity.rs` derive Agent work directly from Herdr `Inventory` state transitions (`status.state == "working"` and state != `"finished"`, minimum 2 seconds). Completed intervals flush during runtime `tick()` and server shutdown; live open `working` intervals are merged on query.
- **Storage, RPCs, and multi-host merging**: Segments persist in SQLite schema `v21` table `activity_segments` without foreign keys (so deleted Cards/Projects keep historical durations) and prune automatically after 400 days alongside event pruning. `activity.record` routes strictly to the local host (`canban-ui` also falls back gracefully on remote/deleted project IDs), while `insights.activity` (`<= 8` days, boundary-clipped + live open intervals) and `insights.activity_buckets` (`<= 400` boundaries) query all connected hosts and merge local dwell with per-host Agent activity. UI ↔ AppServer compatibility uses `DESKTOP_API = 76`.
- **Intentional omissions & scope**: Commit counts and token/cost usage statistics are intentionally omitted (Herdr exposes only process/agent status transitions without usage or token accounting). Dwell records active Canban window focus (`Me · Card`, `Me · Terminal`, `Me · Other`) and does not track time spent in external tools or distinguish finer-grained `agy` screen-refinement sub-states beyond Herdr's `working` state.

## Verification surfaces

Gallery scenes `insights`, `insights-weekly` (`7D` multi-day view), `insights-agents-model` (`Model` grouping view), `insights-empty`, `insights-short-history`, and `insights-loading` (fixed-height cold-load skeleton preview) use a fixed clock (`INSIGHTS_NOW`) and production components with synthetic dwell/agent activity (`fixtures::insights_activity`), bulk initial-sync fixtures, multiple Profiles/Models, and two Projects. Unit, storage, AppServer, dwell, and UI mount tests cover schema v21 migrations, `Inventory` working transitions, `activity.record` / `insights.activity` / `insights.activity_buckets` RPCs, `DwellMachine` idle cutoffs and flush re-queueing, model aggregation, timeline hour windows, and interactive range/group/metric/24h/inspect switching.
