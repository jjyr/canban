# Compact navigation and Quick Open

Implemented 2026-11-07; navigation refined 2026-11-08. This follows the user-selected icon sidebar and fuzzy search scope. The shortcut is **Command+O** on macOS. Clicking anywhere outside the popup dismisses it; Escape closes it immediately, including with a nonempty query. No contextual preview is included.

## Interaction contract

The sidebar is a 56-point rail with 40-point icon targets. Its top-to-bottom order is Quick Open, Board, global Overview, Notes and Archive, with Settings pinned to the bottom. There is no brand letter, separate Project picker, New Project or connection-check action in the rail. Herdr connection diagnostics remain in Settings. Selected state, keyboard focus, accessible names and hover tooltips accompany the icons. Overview retains its global actionable-row badge.

The Board header owns a Project-name button with a chevron. It opens the same search popup restricted to Projects, with a Project-specific placeholder, a checkmark/current label for the active Project and a persistent **New Project** footer action even when no matches remain. New Project closes the popup and opens the existing creation form. Command+O switches an open Project picker into global Quick Open, removing that footer action and current marker. Picking a Project opens its Board, clears the previous Card/Note selection and updates the visible Project name. Empty Boards provide one fully clickable **Create a Project** card; scheduling statistics, task filters and **New Card** stay hidden until a valid Project is selected. Long Project names truncate while preserving the chevron and Card action. Notes and Archive identify their current Project; Overview remains global.

```text
Rail                 Board header
Quick Open           [Current Project v]                 [+ New Card]
Board                   |
Overview [attention]    +--> Project search popup
Notes                       existing Projects; current marker
Archive                     + New Project (persistent footer)
   ...
Settings
```

All icons use vendored Lucide 0.468.0 SVGs through NSH's native vector renderer. Original 24-unit geometry and 2-unit rounded strokes render at 20 points (about 1.67-point strokes). Normal icons use muted text, selected/hovered icons use the theme accent, selection has a soft accent surface, and keyboard focus has its own border. All icons still have 40-point targets and readable accessibility labels/tooltips. The same sources render Quick Open rows and Gallery controls; upstream licensing is preserved in the assets, third-party notices and packaged licenses.

Quick Open searches local metadata across the current Canban home's Projects:

| Result | Searchable metadata | Activation |
| --- | --- | --- |
| Project | Name and directory | Open its Board |
| Card | Number/title, Project, tags and branch | Open Card detail in its owning Project |
| Note | Title and owning Project | Open that Note's editor |
| Agent | Existing Card/profile conversation, profile command name, Card/Project and state | Attach its exact live Terminal, or open its retained Overview context for explicit Resume/recovery |

Result types are also searchable words. A candidate has an icon, primary title and concise ownership/status labels. Duplicate names and Project-local Card numbers remain distinguishable. Search does not index full Note bodies, reports or terminal output.

Typing performs case-insensitive Unicode-aware fuzzy matching: every query term must match a field; exact, prefix and contiguous primary-title matches lead subsequences and secondary metadata matches. Ties use recent choices, visible title/ownership and stable identity. Empty search can show recently chosen destinations first. The last 32 Quick Open choices are remembered within the current UI session.

Up/Down selects candidates and scrolls the selection into view; Enter opens the selected destination. A pointer can choose a row directly. An empty match list shows a clear message. Clicking inside the popup does not dismiss it; clicking outside dismisses without activating a covered control. Dismissal retains the previous page/drafts and restores its input focus through NSH's modal lifecycle. No query or shortcut is forwarded to the underlying Terminal.

Navigation is read-only with respect to execution and workflow. Searching, highlighting or opening a candidate does not enqueue work, resume an exited provider, accept a result or approve a human gate. Activation resolves the current product target again instead of using an old terminal address from the result list. Archived/unavailable Agents retain existing recovery restrictions. Existing Board/Overview return behavior remains; general Back/Forward history and working-tool tabs are separate future features.

## Component ownership

```text
App snapshot + session-local recent choices
                |
       ui/quick_open/model
       typed destinations + fuzzy ranking
                |
       UI Kit SearchDialog / SearchChoice
       input + NSH Command list + NSH Dialog
                |
       fresh destination resolution
       existing Project/Card/Note/Agent navigation
```

- `ui-kit/icons.rs`: shared Lucide SVG icons and shared accessible icon action, with selected/focus/hover/badge styling.
- `ui-kit/search_dialog.rs`: generic popup, controlled placeholder, current-item indicator, caller-composed footer, candidate row and keyboard adapter. It has no product/AppServer/Core dependency.
- `ui/rail.rs`: page-only navigation composition and global badge; Project operations belong to the Board header.
- `ui/quick_open.rs`: shortcut, Project/global mode, query state and destination activation.
- `ui/quick_open/model.rs`: pure local candidates, identities and fuzzy ranking.

NSH Command supplies virtualized candidate rendering, pointer handling, highlight accessibility and scroll-to-selection. Its default search behavior clears a query on Escape, so Canban supplies a controlled input and thin keyboard adapter to deliver immediate dismissal. Search-dialog focus and backdrop behavior use NSH Dialog. Querying performs no disk, Git, RPC or Agent invocation.

The standalone Gallery's **Quick Open** scene imports production components and provides multiple Projects, duplicate names/numbers, Unicode titles, live/idle/queued/closed/unavailable Agent contexts and a long candidate list. The global Command+O shortcut works in the Gallery as well.

## Validation

- Rust regressions cover subsequence and multi-field/Unicode matching, missing results, excluding full document text, cross-Project duplicate identity, conversation deduplication, Project-only mode, recency versus match quality and visible-owner ordering.
- Native Gallery checks cover Command+O, candidate highlight, keyboard and mouse activation, cross-Project duplicates, nonempty-query Escape, outside-click dismissal, retained draft/focus, list navigation and light/dark/constrained layout.
- Isolated packaged app/Core evidence is under `/private/tmp/cb-native-vfaww1lk`. Real Herdr runs used deterministic provider fixtures, not paid/live provider tasks. Cross-Project Note activation, Project picker, Unicode query, live Agent attachment and closed Agent inspection were exercised through native UI.
- `quick-open-before.json` and `quick-open-after.json` preserve the three-request/three-attempt snapshot comparison. Search/navigation added no invocation, retained the Review gate and sent no search input into the running fixture's Terminal.

Chinese IME composition remains excluded as requested. Unicode search was checked through the model and clipboard insertion. This work does not introduce navigation history, contextual quotations, advanced Review, new provider protocols or changes to scheduler admission.

Final checks passed: `mise run test` (62 Rust tests), all-target Clippy with `-D warnings`, main application and Gallery builds, formatting and `git diff --check`. The Gallery's 70-result fixture retained the selected 26th result in view after keyboard navigation in dark appearance; the constrained workspace remained usable. Native Command+O was also tested while a live Terminal owned keyboard focus. No application execution data was changed by search/navigation.

## Navigation refinement acceptance (2026-11-08)

The Gallery adds production icon states (muted, selected, attention) and an Empty Board scene; all routes now compose the production screen entry point, including New Project and Settings. This refinement changes navigation/visual composition only; scheduler/runtime behavior is unchanged. Validation completed with the production Gallery and an isolated packaged app at `/private/tmp/canban-rail-yxbr4s02`:

- Created two Projects through the actual UI: one from the empty Board, the second from the Project selector's New Project footer. The source directories were created and the selected Board updated.
- Filtered the selector and used Enter to switch back to the first Project; verified the visible/accessible Project name and current-item marker.
- Switched an open Project selector to global Quick Open with Command+O; the placeholder changed and the creation action/current marker disappeared.
- Reproduced a focus-related Escape failure after that mode change. Escape now belongs to the mounted modal rather than the input-focused key scope. The same nonempty-query sequence closes on the first Escape, and Escape also closes after Tab leaves the input.
- Clicked actual coordinates outside the popup; it closed without navigating the covered page. New Project remains visible with zero Project matches.
- Verified Project identity in Notes/Archive and retained Herdr diagnostics in Settings. No diagnostic was invoked against a live Core.
- Checked shared SVG icons and attention/selection styling in light/dark Gallery appearance. A long multilingual Project name initially overlapped New Card in the constrained workspace; bounded header width, flexible truncated text and a nonshrinking Card action fixed it. Reran the exact long-title/constrained scene in both appearances, confirming the ellipsis, chevron and Card action remain visible.
- `final-snapshot.json` retains two Projects and zero Cards, requests or attempts. No Agent was launched; no live user home/Core was modified. The task-owned native apps were closed after verification.

`mise run test` passed all 62 Rust tests; all-target Clippy (`-D warnings`), application/Gallery builds, formatting and whitespace checks passed. Chinese IME remains excluded as requested; Unicode names were entered using clipboard paste. This is navigation acceptance, not new Agent/provider workflow acceptance.

The subsequent [Project-entry and Settings refinement](workspace-entry-and-settings.md) replaces the disabled New Card/extra-button empty state with one clickable entry, adds the native folder chooser, removes Overview filters and unifies Settings scrolling. Its current contract supersedes the earlier empty-state acceptance details above.
