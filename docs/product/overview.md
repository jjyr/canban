# Global Activity & Notifications (superseding standalone Overview)

Current contract, updated 2026-11-14: Global cross-Project execution visibility and human intervention live in the persistent **top-right Activity & Notifications trigger and right-anchored popover** (`Command+I`, also searchable in the Command Palette via `>` / `Command+/` / `/`), visible across every page in the unified 42-point window chrome—including `Terminals`. The standalone Left Rail `Overview` page is removed so users never take an extra full-page detour merely to read a status notice and click through to a Card or Terminal.

Board remains the management workspace; Card is the unified side-by-side discussion/problem-solving workspace (`Card + Live Terminal`); `Terminals` owns multi-tab PTY sessions, manual Shells and standalone Managed Branch Worktrees.

## Chrome trigger and popover layout

```text
Window top chrome (42 pt, right end on every screen):
  ... [page actions]   [ ⚡ 2 · ● 3/8 ]
                              |
                              v
  +------------------------------------------------------------------+
  | Activity & Notifications  [3/8 running]          [✓ Read all] [✕]|
  | [ All ]  [ canban · 2 ]  [ tools · 1 ]                           |
  +------------------------------------------------------------------+
  | NEEDS YOU · 2                                                    |
  | ┌───┐ Permission required · Allow `cargo test`? 3m [Card & Term→]|
  | │ 🛡 │ #42 Search permissions · canban · @developer               |
  | └───┘                                                            |
  | ┌───┐ Ready for review · Completed changes...   [Merge][Review →]|
  | │ 👁 │ #28 Export results · tools · @reviewer                     |
  | └───┘                                                            |
  |                                                                  |
  | UPDATES · 1                                                      |
  | ┌───┐ @developer replied · Latest unread reply… [✓ Read] [Card →]|
  | │ 💬│ #31 Fix reconnect · fiber                                  |
  | └───┘                                                            |
  |                                                                  |
  | RUNNING · 3                                        [Terminals ↗] |
  | ┌───┐ Working · Running cargo clippy…          [Card & Terminal→]|
  | │ ✦ │ #36 Add search · canban · @developer                       |
  | └───┘                                                            |
  | ▸ Idle (2) · Closed conversations (8)                            |
  +------------------------------------------------------------------+
```

- **Persistent top-right status pill:** Anchored at the far right of the 42-point window chrome on every screen (`Board`, `Card`, `Terminals`, `Diff & Review`, `GitHub`, `Settings`, and sub-pages). It displays actionable/unread count (`⚡ N` when `Needs you` + unread `Updates` > 0) alongside Scheduler execution usage (`● active/max`). Clicking the pill or pressing `Command+I` toggles the right-anchored popover.
- **Event-first visual hierarchy & semantic icon badges:** Each row leads with a 28×28 tinted semantic icon tile (`ui-kit::tone_icon_badge`: `ShieldAlert` for permission waits, `CircleHelp` for terminal input/sign-in/answer prompts, `CirclePause` for blocked task decisions, `Eye` for human review gates, `GitMerge` for paused/conflicted merges, `TriangleAlert` for failures/recovery, `MessageSquare` for `@agent` replies, `Check` for completed stage reports, `Undo` for Backlog returns, and provider/workflow marks with a live corner indicator for active sessions). Line 1 highlights **what happened** (`Semibold` semantic headline + cleaned single-line summary without raw prefixes like `completed:` or `permission: tool —`), while Line 2 quietly provides **where it happened** (`#number title · project · @profile`, omitting the repeated Project name when filtered to a single Project).
- **Dismissal and input gating:** Escape, clicking outside the popover, or activating a navigation destination closes the popover cleanly and restores focus. While open, underlying native `canban/terminal` keyboard input is gated so shortcuts and navigation keys never leak to a live PTY.
- **Sections and scoping:** Optional Project filter (`All` by default, or scoped to a specific Project with per-Project alert counts). Sections are `Needs you`, `Updates`, `Running`, `Queued`, and collapsible `Idle` / `Closed` groups. Empty groups are omitted. `Running` includes Project-owned Workflow configuration Agents as well as Card Agents; manual Shells remain in `Terminals`.
- **Inline handling & 1-step direct destinations:**
  - `Needs you` combines human Review gates, runtime permission/input waits, and task/recovery decisions while preserving each source's independent resolution. Runtime permission/question items offer **Card & Terminal →**, which opens the Card workspace with its right-hand live Terminal split already expanded and focused on that exact Agent PTY—eliminating multi-hop navigation. Review gates expose **Review →** (and quick **Merge** when local quick-merge is eligible).
  - `Updates` displays unread Card reply/report excerpts (excluding Cards already shown in `Needs you`, which carry their unread count inline). Users can mark individual items read (**✓ Read** via `notifications.read`), mark all unread updates read (**✓ Read all** via `notifications.read_all`), or jump directly to the Card Timeline event (**Card →**). Opening the popover alone never marks items read or approves a human gate.

## Ownership and validation

`crates/notifications/src/overview.rs` retains the authoritative Card conversation, unread, and reason projection. `crates/ui/src/overview.rs` provides both the right-anchored global popover (`overview::popover` and `overview::trigger_pill`) and shared production row projections used by Gallery regression scenes, without render-time runtime or Git reads. `crates/ui/src/commands.rs` exposes `Toggle Activity & Notifications` (`Command+I`) and `Mark All Updates as Read` in the Command Palette.

---

## Historical standalone Overview page (2026-11-11, superseded 2026-11-13)

The 2026-11-11 refinement below describes the earlier standalone Left Rail `Overview` route prior to its replacement by the global top-right Activity & Notifications popover. Keep for decision history and earlier acceptance context ([refinement design](../overview-and-terminal-navigation-proposal.md), [refinement acceptance](../overview-terminal-acceptance-20261111.md), [Agent Overview and lifecycle](../agent-overview-and-lifecycle.md)):

- Overview had no default inspector or Terminal preview; compact production rows showed task identity, Project/profile, and a source-derived reason or reported activity.
- Sections were `Needs you`, `Updates`, `Running`, `Queued`, `Idle`, and `Closed`, with `Idle` and `Closed` collapsed by default.
- Row activation and primary buttons navigated to `Review`, `Card`, or `Terminals` with Back/Forward history preservation.
