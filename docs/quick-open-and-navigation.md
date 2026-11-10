# Compact navigation and Quick Open

Implemented 2026-11-07; navigation refined 2026-11-08. This follows the user-selected icon sidebar and fuzzy search scope. The shortcut is **Command+O** on macOS. Clicking anywhere outside the popup dismisses it; Escape closes it immediately, including with a nonempty query. No contextual preview is included.

## Interaction contract

The sidebar is a 56-point rail with 40-point icon targets. Its top-to-bottom order is Quick Open, Board, global Overview, Terminals, GitHub and Archive, with Settings pinned to the bottom. There is no brand letter, separate Project picker, New Project or connection-check action in the rail. Herdr connection diagnostics remain in Settings. Selected state, keyboard focus, accessible names and hover tooltips accompany the icons. Overview retains its global actionable-row badge.

The Board header owns a Project-name button with a chevron. It opens the same search popup restricted to Projects, with a Project-specific placeholder, a checkmark/current label for the active Project and a persistent **New Project** footer action even when no matches remain. New Project closes the popup and opens the existing creation form. Command+O switches an open Project picker into global Quick Open, removing that footer action and current marker. Picking a Project opens its Board, clears the previous Card selection and updates the visible Project name. Empty Boards provide one fully clickable **Create a Project** card; **New Card** stays hidden until a valid Project is selected. Board has no scheduler summary. Its [GitHub filter bar](github-workspace.md) filters source/type/author/state without affecting scheduling. Long Project names truncate while preserving the chevron and Card action. Archive identifies its current Project; Overview remains global.

### Board title-bar Project activity (2026-11-10)

The Kanban workspace uses the existing 42-point window title bar after Back/Forward as a compact Project switcher. It is visible on Board and the wide Board with attached Card details; it is absent from standalone/narrow Card details, Review, Overview, Settings and Terminals. The Terminal split chrome/tabs remain unchanged.

```text
Window title bar
[traffic lights]  < >  [Fiber  (2)] [Pot  (1)] [Shear] [Tools] [Docs] [+4 v]
------------------------------------------------------------------------
Rail | Canban v                                             [+ Add Card]
     | Backlog                 Develop                 Review
```

All other Projects are eligible, including empty Projects and Projects without events. Each has a subtly filled, borderless button with a medium-weight name and an optional small amber numeric badge. Hover slightly lightens the button background. Long names truncate. The current Project remains in the Board header and is excluded from the switcher.

The badge counts **unique active Cards with reminders**: unread Agent replies/final reports or pending permission/input, task/recovery and human Review actions. A Card with several messages, reasons or Profiles counts once. Current Project and other Project buttons share this projection. Cards show their own item count; zero is hidden. A Review column alone, progress and ordinary runtime updates are not reminders. Reading does not approve an action. See [unified reminders](reminders.md) for identity, deduplication, read positions and sound.

On entry, recently selected Projects come first, then names (case-insensitive, stable ID ties). Recency is window-local and shares the existing Quick Open choice history. Selecting a Project records the departed and selected Project so two-Project switching remains quick. Background events only update badges; they do not reorder existing targets. Newly discovered Projects append until deliberate page/Project navigation creates a fresh order. Badge space is reserved even at zero, so a new permission request does not shift neighboring buttons.

Startup restores the **last opened Project** by stable ID from `config/desktop.json`, entering its Board rather than restoring a Card/Review/Terminal page. Every valid Project-context change, including picker/chip selection, Card navigation and Back/Forward, updates this preference asynchronously. Saves are serialized and newer pending selections replace older ones. The first snapshot and saved preference are read together; a manual selection already made while loading wins over startup restoration. If the saved ID is absent, use the first available Project; without Projects, show the existing creation entry. Theme changes preserve the Project field, and selection changes preserve the theme. Existing configs without this field keep the previous fallback. Only this last Project ID is persisted; full MRU and navigation history remain window-local. Restoration never schedules/resumes Agents.

Implemented in `ui/projects/selection.rs` and AppServer desktop preferences (API level 5). Native acceptance at `/private/tmp/cb-last-project-oy0p3r93/` performed four UI/service boots: first select Beta and quit from its Card details; restore Beta's Board; change to Zeta while retaining Nord and restore Zeta; inject a stale saved ID in the isolated config and verify fallback to Alpha. All four phases retained zero execution requests. `evidence.json` contains saved preferences after each phase. The final test app/service/Core were stopped. All 148 workspace tests, strict Clippy, application build, general isolated acceptance and development-startup acceptance passed. General home: `/private/tmp/cb-roaxcmcg/home`; development home: `/private/tmp/cb-dev-start-9ura6asc`.

The number of visible Projects is determined entirely by available width, bounded name widths, gaps and room for window dragging. There is no three-Project cap and no event-based eligibility rule. All Projects are shown when they fit; overflow space is reserved only when needed. **+N** counts undisplayed other Projects. When no Project fits, **Projects · N** remains as the compact entry. Both open the existing searchable Project picker. The picker shows the same numeric badges, a current marker and New Project action; only duplicate names need a directory subtitle. No blank status subtitle or zero badge is rendered. Fuzzy search still takes precedence over visit order when a query is entered. Global Command+O retains its existing metadata search.

Clicking either the name or badge invokes the same fresh-target Project navigation as the picker. Back restores source context, retained drafts and keyed scroll positions. Navigation does not launch an Agent, change workflow stages, read notices or accept human gates. Badges disappear when the underlying reasons are resolved, not on navigation. The switcher uses existing snapshots and adds no timer, remote/Git query or event-history scan.

Ownership: `ui/board/activity/model.rs` computes unique attention counts, visit/name ordering and responsive width budgets. `ui/board/activity.rs` owns stable keyed Project buttons and switching intents. `ui/frame.rs` mounts the strip only when Board is visible. `ui/quick_open.rs` shares fresh selection, choice history, search and dismissal. `ui-kit/status.rs::attention_count` supplies the domain-free numeric badge; `ui-kit/search_dialog.rs` shares it with Project search rows while NSH owns Button/Command/Dialog behavior.

#### Numeric Project switcher acceptance

Executed 2026-11-10 on the combined working tree: application/Gallery builds, all 146 workspace Rust tests, strict workspace/all-target Clippy, `mise run acceptance` and `mise run acceptance-dev` passed. Layout/count regressions cover all-Project eligibility independent of events, visit/name ordering, unified Review/input counting across multiple Profiles/reasons, archived/removed exclusion, more-than-three capacity, overflow-only-when-needed and stable widths across badge changes.

Native production-component Gallery checks at 1200 / 720 / 440 logical points displayed 7 / 3 / 1 other Projects respectively, folding the remainder into searchable overflow. Empty Projects were directly switchable. Pot showed 2 for two separate actionable Cards, while unblocked working/queued Projects had no badge or status text. The Project picker showed matching badges without empty subtitles/zero counts, and search/Enter, current-Project exclusion and MRU return switching worked. The shared Shortcuts dialog retained its existing labels and Escape dismissal.

The isolated production app at 1360 logical points displayed 9 other Projects. Real Herdr PTYs ran deterministic fixture Agents. Shear's runtime permission changed its badge 0 → 1 → 0 without shifting/reordering its button; Pot's business blockage plus runtime permission on one Card remained 1. Clicking the badge itself navigated to its Project. Overflow search/Enter, direct empty-Project switching, recently departed Project placement and Back restoring a scrolled Board were verified. Requests remained 3 and reports remained 2; Card records/human gates, legacy Notes and existing notice read states were unchanged by navigation. Permission events generated the expected runtime notices.

Local temporary evidence: `/private/tmp/cb-switcher-8hazlgfp/` (`before.json`, `after-permissions.json`, `after-resolution.json`, `after-native.json`, `checks.json`, `evidence.json`). General acceptance used `/private/tmp/cb-shzrzq8q/home`; development startup acceptance used `/private/tmp/cb-dev-start-y0ghdahc`. This is deterministic native/runtime acceptance, not a new live Codex/Claude run or frame-rate benchmark. Test apps/Core were stopped; Chinese IME remains excluded.

#### Earlier summary-strip acceptance (superseded presentation)

The following evidence covers the earlier event-summary/three-entry presentation, before the compact numeric Project switcher superseded it. It remains historical evidence, not acceptance of the current appearance/count/order rules.

Executed checks: production-component Gallery at 1200, 720 and 440 logical points verified direct switching, fewer visible summaries, complete collapse to **Projects · N**, long Unicode Project selection, shared searchable overflow/Enter activation and Escape dismissal. The existing Terminal split chrome was checked without Project summaries. Unit regressions cover Card-count deduplication, human gates and archived exclusion through Overview, meaningful event recency versus deferred Note/runtime/idle/reorder noise, stable ordering and reserved widths across status changes.

An isolated production app with six Projects and deterministic Agents over real Herdr PTYs verified runtime permission events updating **Working → Needs you** without reordering the existing entries. It also verified direct navigation, overflow search to a quiet Project, Back restoring Board scroll, and Back restoring Card details with an unsent comment draft. Navigation retained all three original requests, both reports, all Card records/human gates and existing notice read states. The permission event generated one expected new notice; opening a Project did not resolve it. Local temporary evidence: `/private/tmp/cb-project-activity-m529xvm9/` (`before.json`, `after-runtime-event.json`, `after-navigation.json`, `checks.json`, `evidence.json`). The test app/Core were stopped afterward; this is deterministic navigation/runtime acceptance, not a new real-model acceptance.

The combined working tree passed application/Gallery builds, all 146 workspace Rust tests, strict all-target Clippy and general isolated acceptance. Chinese IME remains excluded. These checks do not establish a frame-rate benchmark or persistent navigation history across application restarts.

```text
Rail                 Board header
Quick Open           [Current Project v]                 [+ New Card]
Board                   |
Overview [attention]    +--> Project search popup
                        existing Projects; current marker
Archive                     + New Project (persistent footer)
   ...
Settings
```

All icons use vendored Lucide 0.468.0 SVGs through NSH's native vector renderer. Original 24-unit geometry and 2-unit rounded strokes render at 20 points (about 1.67-point strokes). Normal icons use muted text, selected/hovered icons use the theme accent, selection has a soft accent surface, and keyboard focus has its own border. All icons still have 40-point targets and readable accessibility labels/tooltips. The same sources render Quick Open rows and Gallery controls; upstream licensing is preserved in the assets, third-party notices and packaged licenses.

Quick Open searches local metadata across the current Canban home's Projects:

| Result | Searchable metadata | Activation |
| --- | --- | --- |
| Project | Name and directory | Open its Board |
| Card | Number/title, Project and branch | Open Card detail in its owning Project |
| Agent | Existing Card/profile conversation, profile command name, Card/Project and state | Open its conversation in Terminals; attach its live PTY or show retained context for explicit Resume/recovery |

Result types are also searchable words. A candidate has an icon, primary title and concise ownership/status labels. Duplicate names and Project-local Card numbers remain distinguishable. Search does not include deferred Notes, full reports or terminal output.

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
       existing Project/Card/Agent navigation
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

Terminal navigation update (2026-11-08): all Agent results now use the [dedicated Terminal workspace](terminal-workspace.md), including queued/closed/archived records. The former closed-result Overview-inspector route in the historical acceptance above is superseded. Browsing remains read-only; explicit Resume remains separate.

## Terminal tabs

Quick Open now also indexes saved manual Terminal names/numbers and directories. Opening an Agent or Terminal destination reuses its existing tab/binding without launching work. Command+T creates a Shell in the selected Project/Card context; Command+W terminates its live PTY and closes the tab; Command+Shift+[ / ] cycles opened tabs. Search and rename dialogs suspend these shortcuts and restore native Terminal focus on dismissal. The left inventory and right tab strip extend separately into window chrome.

## Unified window history (2026-11-09)

All pages now share Back/Forward controls and Command+[ / Command+] through navigation.rs. These replace separate source-return flags; route traversal preserves window-owned drafts/scroll state and never repeats operations. See [current implementation and native evidence](terminal-interaction.md).
