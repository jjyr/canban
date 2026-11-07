# Compact navigation and Quick Open

Implemented 2026-11-07. This follows the user-selected icon sidebar and fuzzy search scope. The shortcut is **Command+O** on macOS. Clicking anywhere outside the popup dismisses it; Escape closes it immediately, including with a nonempty query. No contextual preview is included.

## Interaction contract

The sidebar is a 56-point rail with 40-point icon targets. It contains Quick Open, global Overview, a Project picker, Board, Notes, Archive, New Project, connection check and Settings. Selected state, keyboard focus, accessible names and hover tooltips accompany the icons. Overview retains its global actionable-row badge.

The Project picker uses the same search popup, restricted to Projects. Command+O switches an open Project picker into global Quick Open. Picking a Project opens its Board and updates the rail's Project label.

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

- `ui-kit/icons.rs`: original line icons and shared accessible icon action, with selected/focus/hover/badge styling.
- `ui-kit/search_dialog.rs`: generic popup, input, candidate row and keyboard adapter. It has no product/AppServer/Core dependency.
- `ui/rail.rs`: product navigation composition and global badge.
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
