# App UI review and design directions

Date: 2026-11-07. This document preserves the pre-redesign audit and alternatives. Direction A and the workspace hierarchy were subsequently approved and implemented. The audit below describes the earlier UI, not the current component inventory.

Current implementation: Board plus optional Card pane; focused Card/Terminal or Review; shared semantic navigation, status and split controls; SQLite-backed Notes with editing/preview, Timeline and bidirectional links. See [current product layouts](product/notes-and-workspace.md), [component inventory](ui-components.md) and [implementation/acceptance](notes-and-workspace-implementation.md). The user clarified that file-like Notes remain in the database; no physical-file migration is involved.

## Evidence and current structure

The audit reads the current working tree and inspects the packaged native app in a disposable Canban home with an isolated Herdr Core. Board and Card detail were inspected visually and opened through native controls. Other observations below come from source; this is not full interaction acceptance. Chinese IME checks are excluded at the user's request.

```text
Desktop shell
  Sidebar: Projects / Board / Notes / Agent activity / Archive / Settings
  Main route: Board, Card, Notes, Agent activity, Review or Terminal
  Bottom status text

Board -> Card detail -> Review / Terminal (separate full routes)
           |
           + Task editor and Markdown preview
           + Card Agents
           + Code delivery
           + Human Context selection / Resources
           + Timeline / Comment composer
           + Request panels / Output / Archive action
```

| Owner | What exists | Main limitation |
| --- | --- | --- |
| `crates/ui-kit/src/tokens.rs` | Adaptive light/dark colors, radius 8, control height 34 | Typography, spacing, density and interaction-state roles are not yet a complete token system |
| `crates/ui-kit/src/button.rs` | Primary / Secondary / Danger / Quiet on NSH Button | All variants have a border; Quiet still looks like a boxed action; hover dims the entire control |
| `crates/ui-kit/src/surface.rs` | Panel, heading, body, muted text, badge | Panels share one border treatment; all badges are neutral regardless of meaning |
| `crates/ui/src/screens.rs` | Sidebar, Board, Card, Notes, activity, archive, settings | Large screen composition module; navigation uses ordinary actions without selected-route styling |
| `crates/ui/src/components.rs` | Card tile, Timeline item, run/output panel, context picker | Several independent components share one file; reusable behavior exists but presentation contracts remain small |
| `crates/ui/src/sessions.rs` | Card/profile entries and Terminal route | Profile list, run list and Terminal toolbar repeat related information; state strings expose runtime details |
| `crates/ui/src/review.rs` | Diff preview, target/message fields, explicit acceptance | Diff is Markdown; controls occupy considerable vertical space; changing routes separates discussion from code |
| `examples/gallery` | Shared production controls, profile picker, Timeline item, Diff, static Terminal | No complete Board/Card workspace scene or complete component-state matrix yet |

The existing UI Kit/product/NSH boundary is suitable. The next step is to extend useful components and improve composition, retaining that boundary. See [UI architecture](ui-architecture.md) and [component inventory](ui-components.md).

## Main opportunities, in priority order

### 1. Make the next human action visible

The Board should answer: what is running, what is waiting for me, and what is ready for review? Today a Card has a text execution summary and an attention label, while unread notices are only a count in the sidebar. There is no actionable notification list in the current screen implementation.

Give each Card a consistent hierarchy: number/title, short optional summary, current Agent/activity, attention or next action, then secondary tags. Reserve strong emphasis for attention and the relevant action. The default local backend does not need a persistent prominent Local badge. Display secondary paths and request/attempt IDs in inspectable details rather than primary product text.

Keep three independent meanings: workflow stage (Backlog/Develop/Review), runtime (running/idle/offline/closed), and human attention (answer needed/failure/review ready). Use text plus an icon/shape; color alone must not carry meaning. A Review Card can still have a running follow-up Agent.

### 2. Replace the long Card form with a working view

In the observed Card screen, always-visible editor fields, a duplicate rendered description, full-width Save and Review buttons, and multiple boxed sections push Timeline and comments below the first viewport. A routine progress check requires scrolling past editing tools.

Proposed default: readable task content, visible attention/Agent summary, Timeline and an accessible composer. Enter editing deliberately; show save/dirty/conflict feedback near that editor. Put context selection, attachments and metadata in a secondary region or disclosure. Keep one prominent action for the current task, with archive and uncommon actions in an overflow menu.

The existing Card Agents entry and request/attempt panels should share one conversation summary component, with expandable execution history. Opening a live terminal attaches its viewer; opening an exited conversation resumes it. Labels should explain which action will happen. This changes presentation only; the Card/profile conversation model remains the same.

### 3. Preserve place across Board, Card and Terminal

Today Card, Review and Terminal replace the main route. A promising desktop arrangement is a Card detail pane, with Terminal or Review expandable when needed. Keep Board scroll, selected Card, drafts and active profile when returning. A dedicated Terminal view remains useful for long sessions; it need not be the only layout.

```text
Board browsing (wide window)
+-----------+----------------------------+------------------------+
| Projects  | Project / Board toolbar    | #42 Card header        |
|           | Backlog | Develop | Review | Task / Timeline        |
| Board     | cards   | cards   | cards  | Agent summary          |
| Notes     |        selected Card ------> Context / resources    |
| Agents    |                            | Comment + @profile     |
+-----------+----------------------------+------------------------+

Focused work (Terminal or Review opened from the selected Card)
+-----------+----------------------+------------------------------+
| Projects  | #42 Task + Timeline  | Terminal: @developer         |
|           | Context / composer  | or Review: report + Diff     |
| Board     |                      |                              |
+-----------+----------------------+------------------------------+
```

These are layout options, not a requirement to keep every pane visible. Start with one resizable split; avoid building an IDE docking framework. At narrower sizes, switch to one main surface with a clear back action and preserved state. Check actual useful minimum widths with long titles and multiple Cards before selecting breakpoints.

### 4. Give Timeline and replies a human reading order

The current Timeline emphasizes sequence/actor and raw event-kind strings; it does not format timestamps in the row. Comments render Markdown, while lifecycle events use ordinary labels. Each event can repeat report/output actions. The composer follows the entire history.

Use distinct presentations for human/Agent comments, concise workflow events, result reports and attention requests. Show author/profile and time; expose raw event metadata on demand. Consecutive low-value runtime events can collapse visually while remaining in durable history. Keep the latest report and unanswered question easy to locate. Keep the composer reachable; preserve position when reading earlier entries and offer a new-activity indicator instead of forcing scroll to the bottom.

An @mention appended to a running Agent needs visible sent/pending/replied/error feedback. A terminal response and a recorded CLI reply have different provenance; do not manufacture a Timeline reply from arbitrary terminal text.

### 5. Make Board movement understandable

Current movement uses a dedicated pan handle and translation/column-width calculation; it is not a complete drag-preview/drop-target/autoscroll interaction. Retain an explicit Move action as a keyboard-accessible alternative. Add a clear dragged Card, valid destination highlight and cancellation behavior. When a move schedules an Agent, show the destination's configured action before committing and show queued/running/failure feedback afterward. Normal moves should remain quick; avoid a confirmation on every drag.

### 6. Unify feedback and recovery

Today many operations share one global pending flag/status line. Errors can be distant from the triggering action and later operations replace the text. Show field validation next to its field, operation progress near its action and durable attention on the Card. Use transient success feedback sparingly. Make unread notifications actionable and route them back to the relevant Card.

Terminal controls should prioritize the current state: Open when live, Resume when exited, reconnect feedback when unavailable. Interrupt, end and fresh-context recovery have different effects and need separate labels and eligibility. Show Core availability separately from Agent execution and business completion. A disconnected viewer must not look like a completed task.

Review should put report and changed files before delivery configuration, and show why acceptance is unavailable. Return-to-Develop should preserve the user's rework explanation. Acceptance explicitly commits/merges; Archive stays a separate product action. Keep the existing server validation and preview-token safety when changing the layout.

## Visual directions

The references describe broad interaction and composition qualities; they do not imply copying assets or adopting another product's complete UI.

| Direction | Visual language | Interaction emphasis | Tradeoff |
| --- | --- | --- | --- |
| A. Calm native workspace | Neutral adaptive surfaces, restrained blue accent, subtle separators, compact sidebar rows, clear system typography; closer to native macOS productivity tools | Board and human attention first; secondary tools appear when needed | Less information per viewport than a dense console; requires careful hierarchy to avoid looking generic |
| B. Compact engineering workbench | Tighter rows, stronger panel boundaries, persistent execution indicators, monospace only for code/IDs; familiar editor/workbench density | Simultaneous Card, Agent and Diff inspection; keyboard shortcuts | Can make a planning/context product feel like an IDE and increase visible complexity |
| C. Context notebook | Softer neutral surfaces, wider reading measure, generous task/report typography, minimal chrome; document-oriented | Notes, task reasoning, conversation and readable reports | A sparse notebook treatment can hide scheduling state and reduce Board scanning efficiency |

Recommendation for discussion: choose A as the product's base. Give the Board moderately compact Cards and give task/report content a bounded reading width. Terminal and Diff may use denser content appropriate to their job, while navigation, controls and state meanings remain consistent. Dark mode should use the same hierarchy and semantics, not become a separate visual style.

Avoid equating a style choice with a palette choice. First decide information density, primary workspace layout and emphasis; then compare typography, spacing, borders and accent in the same realistic Gallery scene.

## Component design language

Extend the existing layers incrementally:

```text
UI Kit: semantic tokens + NSH behavior
  NavigationItem / Button variants / StateIndicator / Field + feedback
  Toolbar / Section / Tabs / Menu / SplitPane / EmptyState / InlineNotice
                         |
Product components
  CardTile / StageColumn / ConversationSummary / TimelineEntry
  CommentComposer / ContextResources / ReviewSummary / TerminalToolbar
                         |
Screens and presenters
  selection, drafts, pending/error state, navigation and AppServer intents
```

Candidate components above are needs to evaluate, not a request to implement every wrapper. Reuse suitable NSH facilities; add a Canban component only when it carries a real shared visual/interaction contract. Move independently reusable product components into individual modules when changing them.

| Contract | Proposed rule to evaluate |
| --- | --- |
| Surfaces | Distinguish app background, content, inset and overlay; use whitespace/separators before adding another bordered panel |
| Type | Establish page title, section title, body, metadata and code roles; keep body readable, reserve tiny text for low-priority metadata |
| Actions | One clear primary action per working region; Secondary outlined; Quiet genuinely low-emphasis; separate navigation from actions |
| State | Neutral / informational / success / attention / danger roles, with text/icon; stage, runtime and attention remain independent |
| Geometry | Named spacing/radius/control-size roles; allow compact metadata and comfortable text editing without arbitrary per-screen metrics |
| Focus | Visible keyboard focus distinct from selection, with consistent Tab/Escape/return-focus behavior |
| Editing | Explicit dirty/saving/saved/failed/conflict states; drafts survive async refresh and navigation according to a defined policy |
| Icons | One consistent icon family and optical size; paired labels for consequential or unfamiliar actions |
| Motion | Short feedback for selection/movement, no layout motion that disrupts reading, respect reduced-motion preferences |

## Proposed implementation order after direction selection

1. Add realistic populated Board/Card composition scenes to the standalone Gallery. Include long text, running work, unanswered question, Review ready, failed/offline and empty states. Keep fixtures out of the production host.
2. Establish tokens, navigation selection, action hierarchy and semantic state indicators in shared components. Verify both appearances, focus and representative narrow/wide compositions.
3. Implement one complete Board -> Card -> comment/@Agent -> Timeline -> Terminal path with preserved place, drafts and visible feedback. Validate live AppServer/Herdr behavior in the main app separately.
4. Improve Review composition and Notes reading/editing, then extend notification/activity surfaces. Keep Shear, template GUI and GitHub backend deferred.

Discussion choices: A/B/C base style; comfortable versus compact Board density; Card side pane versus full-page default; Terminal beside Card versus separate view by default. A low-complexity starting recommendation is A, moderately compact Board, wide-window Card pane with a full-page fallback, and Terminal expanded on demand. Validate this in the Gallery before committing to a broad screen rewrite.
