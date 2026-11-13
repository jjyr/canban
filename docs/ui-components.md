# UI component inventory

Status: implementation inventory, current contracts reconciled 2026-11-10. Dated verification entries retain their original scope; this documentation update adds no runtime acceptance. These modules are used by the native application; the Gallery imports the same implementations.

| Component/layer | Module | NSH reuse and contract | Verification |
| --- | --- | --- | --- |
| Semantic tokens | crates/ui-kit/src/tokens.rs | Fixed-dark reactive surfaces/text/accent/borders and sizing from the named-theme catalog, synced into NSH design tokens (`Theme::set_color` / `set_number` / `set_font`) so NSH Tailwind token classes (`bg-border`, `bg-ring`, `bg-primary`, etc.) stay reactive across named-theme switches | Named-theme Gallery/application acceptance in desktop-utilities.md and `ui-kit::split` token unit tests; earlier Light/Dark checks are historical |
| Scroll indicators | crates/ui-kit/src/scrollbar.rs | Canban custom theme-reactive overlay scrollbars (`set_builtin_scrollbars` + `vertical_scrollbar`): slim `MUTED` pill thumb, reveal on scroll or rail hover without lighting up on ordinary content hover, thumb dragging and track jump, fade after idle, zero layout width reserved, explicit `.accessible(false)` + `PointerEvents::BoxNone` mount propagation so `scroll()` hit-testing stays unobstructed, and outer-gutter `vertical_scrollbar` placement on Board stage columns clear of Card borders and hover actions. Native Terminal rendering remains separate. | Unit `MountBatch` / theme tests, Gallery `board-overflow` & `controls` scenes, and isolated native App scroll, Card click/drag and theme switch |
| Button | crates/ui-kit/src/button.rs | nsh-base Button; typed variants (`Primary`, `Secondary`, `Quiet`, `Danger`), `compact_action` for 26-point inline/toolbar controls, shared text/icon actions and centralized hover/pressed/focus/disabled styles. `press_target()` is the unstyled press-target constructor for every clickable surface: it shows the pointing-hand cursor while enabled and the arrow while disabled; do not call `nsh_base::button()` directly | Native mouse, Tab/Space, pointer-cursor and disabled-skip checks |
| Tooltip | crates/ui-kit/src/tooltip.rs | nsh-base Tooltip; `tooltip_box()` and `hint_tooltip()` provide the shared opaque `SURFACE` + `BORDER` + shadow bubble across Card tiles, Card headers, chrome buttons, Terminal tabs and sidebars; do not call bare `nsh_base::tooltip()` directly | Unit source guard, Gallery `board-cards` / `controls` scenes, and isolated native App hover checks |
| Text field | crates/ui-kit/src/input.rs | Controlled NSH Input; common geometry/focus/text | Multiline Unicode paste/edit/save, live Markdown preview and retained draft through state refresh |
| Checkbox | crates/ui-kit/src/checkbox.rs | Controlled nsh-base Checkbox; common checked/focus presentation | Reusable control retained; the former Note-selection consumer is removed |
| Numeric attention badge | crates/ui-kit/src/status.rs | Domain-free positive count, hidden zero, 99+ visual cap; shared by Project buttons and search rows | Production Project activity Gallery and native badge updates |
| Borderless section group | crates/ui-kit/src/section_group.rs, crates/ui-kit/src/settings.rs | `section_group` and `collapsible_section_group` compose a borderless header (title, optional count badge, subtitle/description, trailing controls) over a 1px `BORDER` divider and stacked content body; `settings_section` builds on `SectionGroup` with field spacing | Gallery Controls, Settings, Overview, and GitHub Account/Repository scenes |
| Panels/text/badge | crates/ui-kit/src/surface.rs | Shared NSH layout/text composition | Native themes and real Card composition inspected |
| Card tile | crates/ui/src/components/card_tile.rs | Content/surface projection; Board owns whole-Card pan, keyed rows, placement and cancellation | See board-interaction.md for current whole-Card drag acceptance; earlier handle checks are historical |
| Project switcher | crates/ui/src/board/activity.rs and activity/model.rs | Board-only filled Project buttons, unique actionable-Card badges, visit/name ordering, all-Project eligibility and width-driven overflow without a fixed cap | Project activity Gallery scene and isolated native navigation; see quick-open-and-navigation.md |
| Timeline item | crates/ui/src/components/timeline_item.rs | Connected vertical-spine narrative cards & semantic milestone badges, inline `+N earlier` group disclosure, file-backed Markdown reports and Terminal/detail intents | Production and Gallery `Card conversation` / `Controls` fixtures; unit projection & spine tests |
| Agent completion / registry | crates/ui/src/profiles.rs, card/composer.rs | Caret completion and explicit recipient projection; Settings shows role instructions | Native completion/dispatch and ordinary prose exclusion; Board labels reflect template references |
| Agent run/output panel | crates/ui/src/components.rs | Request/attempt view, retry and read-only output | Request history and retained output; interactive Terminal lives in sessions.rs |
| Presenters | crates/ui/src/presenter.rs | Async AppServer operations, event-driven refresh, draft preservation and resource generation checks | Live state updates, native saves and image-paste refresh |
| Product screens | crates/ui/src/screens.rs | Compose production components; application navigation only | Board and Card native checks |
| Standalone Gallery | examples/gallery/main.rs and fixtures.rs | Imports production components from canban-ui; local fixture state without AppServer/data/Core | Run mise run gallery for component checks |

Components use their current typed Rust inputs/events; longer-form component API docs can grow with the public surface. Menus/dialogs/rich-editor/virtual-list wrappers should be added when a concrete feature needs them, using NSH facilities first. Do not create parallel screen copies or claim unimplemented Gallery scenarios as tested. The [design-system conventions](ui-architecture.md) govern subsequent changes.

## Interactive Agent components (2026-11-07)

crates/ui/src/sessions.rs owns the explicit Start/Resume intent; Card entries now navigate through the compact header and the Terminal Choose Agent control. The dedicated Terminal page/tree/viewer now live in crates/ui/src/terminals.rs and terminals/; the earlier Card/Terminal focused split is superseded. Input is the selected request/attempt and profile registry; actions open/resume, interrupt, end or explicitly start fresh context. Navigation detaches only the viewer. crates/terminal owns the native canban/terminal component with immutable binding (config, core ID, terminal address), read_only input gating and an independent stream worker. It has no Card/workflow/storage dependency. The Gallery uses this production native component with a deterministic VT fixture. Completion alone does not gate input: completed live Agents remain interactive until idle reclamation or explicit close. Connecting/Disconnected/Closing explain input gating; uncertain-close errors remain inspectable.

Native terminal regression scenarios also cover hidden cursor/null cursor metadata, wide-glyph background ordering, clipping to native bounds and scrolling a static Gallery fixture into view. The component redraws on native visibility/window-position changes as well as VT revision changes.

## Unified Diff & Review workbench (2026-11-07, updated 2026-11-12)

`crates/ui/src/review.rs` (with `review/model.rs`, `review/sidebar.rs`, `review/diff.rs`, and `review/submit.rs`) owns the first-class Left Rail `Diff & Review` route (`Icon::Compare`) as well as direct Card review/delivery and `diff_panel`. It operates with or without an active Card or Pull Request:
- **Ad-hoc Git Compare**: Top Project, Base, and Compare pickers plus the collapsible Quick Compare sidebar (`Working tree`, `Staged`, `Last commit`, local/remote **Branches**, `Cards & PRs`, and `Recent Commits`) invoke `git.refs` and `git.compare`.
- **Card & PR Review**: Local Cards with a worktree or branch expose `Diff` / `Review` across stages and preserve immutable `ReviewPreview` token-checked Local Merge, Publish PR, merge-blocker notices, `Return to Develop`, `Delivered` receipt + `Archive Card`, and a toggleable `Show Card` drawer. GitHub PR Cards without a local checkout fetch remote PR files and review threads via `review.github_pr` (`gh` CLI / GitHub cache), and offer a `Local branch` / `Remote PR #N` switcher when a local checkout also exists. The Card header also offers a one-click `Merge` (quick merge) for eligible local-delivery Cards, owned by `crates/ui/src/card/quick_merge.rs`; it reuses `review.preview` → `review.merge` and confirms only preview warnings (see [review delivery](product/review-delivery.md#quick-merge-from-the-card-header)).
- **Inline Comments & Batch Dispatch**: Clicking any unified diff line opens an inline comment box attached to `path:line` (`LEFT`/`RIGHT`) and collects a pending batch. Dispatch (`review.send_agent` or `github.remote.review`) supports launching an interactive `@Agent` directly in Herdr Terminals (even without a Card), creating a new Card, posting to a Card's `@Agent` locally (seeding `refs/canban/cards/<id>` from `refs/pull/<n>/head` when needed), or syncing a formal Review (`Comment` / `Approve` / `Request changes`) to GitHub. The standalone Gallery exercises the same production components across `Git Compare`, `Review delivery`, and `Delivered receipt` scenes.

Terminal reconnection keeps the immutable Core/terminal identity, waits for native layout before initial attach, reapplies current size/focus after handshake and drops disconnected/uncertain input. Precise wheel deltas accumulate and are converted to libghostty's negative-up direction. The 12,000-line Model regression and native upward/downward scroll checks cover the shared behavior.

## Standalone Gallery ownership

The development Gallery and its sample records/VT data live exclusively in `examples/gallery`. `canban-ui` exports reusable production modules as a library; its small binary calls the desktop host. Main-app navigation contains no Gallery entry/route, and its startup does not select a development Gallery mode. `mise run gallery` builds a separate Cargo example and `.local/CanbanGallery.app`, with its own bundle ID and no bundled Canban CLI. Component development and regression scenes are checked there; see [example instructions](../examples/README.md).

The native Terminal's optional `content` property replaces a static VT preview when there is no runtime binding; repeated identical content is idempotent. Live bindings still use Herdr. Sample VT bytes are supplied by the example rather than embedded as a production `fixture` branch.


## Kanban workspace and Notes components (2026-11-07)

Historical inventory: Notes were [deferred](notes-deferred.md) on 2026-11-10. `ui/notes.rs` and `ui/components/context_picker.rs` are removed, and no Note navigation/editing/relations surface ships. Shared controls below remain available for current Card/Board/Review composition; the dated Note consumers and acceptance describe the earlier build.

| Owner | Input/state and interaction contract | Composition and verification |
| --- | --- | --- |
| `ui-kit/navigation.rs` | Label, selected predicate, press intent; selected/hover/focus remain separate | Sidebar and Note list share selection styling |
| `ui-kit/status.rs` | Semantic `Tone`, chip/notice text; state remains textual as well as colored | Runtime/attention/result feedback in Board, Card and Notes |
| `ui-kit/split.rs` | `horizontal_group`, `split`, and `reveal_split`: left/right elements, retained width signal, minimum widths, and `HANDLE_CLASS` (`w-px bg-border group-hover:w-[3px] group-hover:bg-ring group-active:w-[3px] group-active:bg-primary`) on NSH's full-height handle indicator; NSH owns resizing/focus/accessibility | Board/Card (`reveal_split`), Card + Live Terminal (`split`), Terminals inventory/viewer (`horizontal_group`), GitHub inbox, and Controls Gallery |
| `ui-kit/input.rs` | `bound_field` reads/writes a structured draft through the shared NSH Input | Card and Note editors keep the same geometry/focus behavior |
| `ui/board.rs` | Project snapshot, selected Card, stage scroll store; open/move intents | Project/work-location/Add Card header, all active stage Cards, horizontal overflow and independent column scroll |
| `ui/card.rs` | Saved Card plus retained draft; read/edit, context disclosure, Agent entries, history and reachable composer | Default reading view; opening work retains drafts and Board position |
| `ui/workspace.rs` | Route, selected Card, viewport, expanded state and split widths | Board/Card or Card/work; narrow mode shows one surface with back/toggle controls |
| `ui/notes.rs` | Note selection/query, saved snapshot, retained draft and related-pane state | Edit/preview, revision/conflict feedback, insert-link, backlinks, related Cards, Timeline and Card creation |
| `ui/drafts.rs` | Root-owned maps keyed by document ID; async save can outlive a route owner | Metadata refresh reconciles revision; remote content never overwrites a dirty draft |
| `ui/components/activity.rs` | Reusable activity header and empty state | Shared product shell without domain dependencies in UI Kit |

`Ui.navigate` invalidates pending resource output on sidebar changes; new Note selection does the same. Native Terminal construction is memoized by run/binding rather than unrelated snapshot changes. Closing the viewer does not issue an Agent control operation. Drafts/scroll live for the app process; they are not crash-persistent storage.

Gallery workspace scenes import the exact production components, including focused Review and Note relations. Its constrained viewport controls exercise narrow composition without changing system display preferences. Current native acceptance and remaining resize/automation limits are recorded in [Notes/workspace implementation](notes-and-workspace-implementation.md).


## Git workspace and scheduler controls

`components/git_workspace.rs` projects the selected Card's branch/path/base/delivery with read-only status refresh and exact-value copy actions. Git reads run off the UI thread and are fenced against disposed routes. Visual labels compact long paths so actions remain reachable in narrow panes. `components/scheduling.rs` supplies the shared global usage/waiting summary and positive-integer limit editor; both Settings and the standalone Gallery use it. Card human gates show an explicit reviewed/continue action rather than treating notice acknowledgement as approval. See [workspace/scheduling](workspace-and-scheduling.md) for native and backend validation.


### Overview components

`ui/overview.rs` owns compact global rows, Needs you/Updates/Running/Queued sections and collapsed Idle/Closed disclosure. Rows consume prepared Card/profile and Project configuration state and emit direct destination intents. Primary actions and row activation share eligibility; explicit Resume remains separate. Task titles truncate with full source context in a tooltip; Profile stays first in the secondary column at narrow widths. Returning restores row focus, selection and scroll. There is no context inspector or embedded Terminal preview. `terminals/navigation.rs` shares Card/profile/request resolution, a native multiple-Agent menu, exact historical context and queued launch navigation with location fencing. Gallery Overview fixtures cover multiple Projects/profiles, simultaneous task/runtime reasons, long multilingual titles, unread replies after long Markdown, disconnected/queued/closed states and 720-point layout. See [refinement acceptance](overview-terminal-acceptance-20261111.md).


### Compact navigation and search

`ui-kit::Icon` and `icon_action` define vendored Lucide native-vector icons and 40-point accessible navigation targets. `ui/rail.rs` composes the 56-point product rail, global attention badge. The Board header owns the Project picker and New Project entry. `ui-kit::SearchDialog` takes prepared `SearchChoice` values (including a current-item marker), controlled placeholder and caller-composed footer and reuses NSH Dialog/Command for the popup and candidate list; product targets and fuzzy ranking stay in `ui/quick_open`. Command+O, immediate Escape, outside dismissal, candidate highlight and exact read-only navigation share the same production components in the Gallery. See [interaction and acceptance](quick-open-and-navigation.md).


### Project entry and Settings components (2026-11-08)

`ui-kit/entry_card.rs` owns the single accessible clickable entry; `directory_field.rs` owns the editable path/native folder sheet with cancellation and disposed-owner guards; `settings.rs` owns generic section/field composition. Product UI owns Project name suggestions and creation. One screen scroll contains Agent scheduling, Agent profiles, Workflow, Herdr and Storage; the scheduler component uses the same section/field geometry. `profiles::registry_items` supplies content without nested panel styling. The Gallery includes Empty Board, Settings, all-state grouped Overview and entry/directory controls at a 720-point constrained width. See [contracts and native verification](workspace-entry-and-settings.md).

The simplified creation presenter lives in `ui/projects.rs`: Directory precedes Name, basename suggestions stop after an explicit name edit, and NSH Collapsible keeps Execution machine secondary. `ui-kit::directory_input` owns the common path/Choose row geometry; `directory_field` supplies its native picker, while `ui/machines::directory_picker` supplies the selected host's server browser. New Project and Work location share that path component, with one chooser action each. Gallery has an explicit New Project entry and machine fixtures for path clearing, custom-name preservation and collapsed remote summaries.


### GitHub Card and unified creation (2026-11-09)

`ui/github.rs` composes account/source Settings, the unified Local/GitHub form and source/PR sections from shared controls. Add Card opens that form directly. Query/result/selection projections isolate reactive updates so search responses do not replace the text input; type changes preserve separate drafts. Long remote descriptions live inside the Card's main scroll area. Card tiles use a compact GitHub mark and source-number tooltip. `ui-kit::action` now accepts reactive labels, and `Icon::GitHub` uses the official MIT-licensed Octicons mark with theme color. The Gallery includes the production GitHub Card scene. See [implementation and validation](github-integration.md); a fixture scene is not live GitHub authorization acceptance.

### Dedicated Terminal workspace (2026-11-08)

`ui/terminals/entries.rs` projects Agent conversations and durable manual Shells into one inventory. `tree.rs` and `tabs.rs` retain native pointer/focus targets across irrelevant runtime timestamp updates. `terminals.rs` owns a full-height resizable split: traffic lights/navigation over the left inventory, and numbered tabs/Card/branch over the native viewer. `ui/frame.rs` is the production frame shared with the standalone Gallery. UI Kit's `chrome_button` provides compact accessible icon controls; all secondary actions use native NSH menus anchored to their trigger or context-menu pointer. Native Terminal focus is explicit and suspended for search/rename; its accessibility value includes connection status and emits throttled change notifications. The permanent technical footer is removed; operation failures use dismissible notices. See [contracts and acceptance](terminal-workspace.md) for manual Shells, unified close-and-terminate, exact Agent resume and validation limits.

Window-owned commands use `Ui::spawn` so runtime refresh, button replacement and navigation cannot cancel publication of a completed receipt or leave `pending` stuck. Page-specific observations remain scoped/cancellable; command-driven navigation and local editors retain source fences. A snapshot epoch rejects refreshes started before a newer command receipt. The native Terminal accepts UI-declared Command keys and suspends PTY input synchronously before the asynchronous UI focus transfer; search/rename/pending actions keep input gated until focus returns.

### Independent Terminal context (2026-11-08)

The production inventory groups entries by task Project (managed Agents), observed Project (independent Terminals), or Other. Card hints are chrome links, not nested inventory groups. `Entry` combines durable presentation identity, optional task association and ephemeral Herdr directory context; the native binding projection excludes directory hints so regrouping cannot remount the PTY. Context subscriptions do not change Board selection or tab order. Card return and rail navigation issue explicit focus tickets. Gallery Terminal fixtures include unassociated and inferred-Card independent Terminals alongside managed Agents.

### Terminal title presentation

`ui/terminals/labels.rs` is the shared product component for provider/Profile marks and bounded tab labels. `Entry` derives automatic titles and the selected Request stage. Agent inventory rows reserve first-line stage and second-line status around a flexible title and Provider/Profile, then show machine name. Independent Terminals show name/status and machine on two lines, without a type icon. Selected titles wrap; unselected titles truncate. Provider/Profile names deduplicate. Tab labels reserve stage/Profile and a separate close button. List rows and their Provider labels have no metadata tooltip or directory text; accessibility labels retain the full inline context. Compact tabs retain their own tooltips. Long branch/directory labels use middle ellipsis; their hover retains the complete value. The Gallery has Codex/Claude, custom providers, Unicode/long Card titles, multiple Profiles and independent numbered entries. UI Kit only supplies reusable provider vector assets and generic rendering.


### Quiet Board Cards and direct type selection (2026-11-09)

`ui-kit/segmented.rs` provides direct single-choice buttons with shared adaptive tokens and separate selected, hover and keyboard-focus states. The New Card form uses Local / GitHub Issue segments, including the official GitHub mark, instead of a type menu. Drafts remain independent across type switches.

Board headers show the workflow stage name without numeric Card counts. When active column rules exist, a workflow-node button opens their details on click; it has no hover tooltip. Headers retain equal height with or without this button. Custom workflow stage names remain unchanged. Card tiles omit repeated stage labels, drag instructions, empty metadata rows and inactive/completed/queued execution summaries. The top-right Grip is now decorative; the [whole-Card drag contract](board-interaction.md) supersedes its earlier dedicated target. Titles remain primary, excerpts are single-line, branch text is bounded with a full tooltip. Routing-tag fields and badges are removed from creation, editing, Board tiles and Quick Open. `components/activity.rs::card_status`/`running_agents` share actionable gate/failure chips and running Agent identity; provider detection is shared with Terminal rows through `profiles::provider`.

The standalone Gallery adds Card states and New Card scenes, including queued, executing, human-gated and long-title Cards at constrained width. Native acceptance at `/private/tmp/cb-native-o55cpz5o` verified small-grip Backlog → Develop drag with actual fixture scheduling, direct type switching with exact title/body retention, and Local Card creation. UI tests (12), targeted strict all-target Clippy, application/Gallery packaging and diff whitespace checks passed. These interaction fixtures do not invoke a model or use the user's live Core.


### Terminal close lifecycle and input state (2026-11-09)

X, Command+W and context-menu Close terminal invoke one AppServer `terminal.close` operation with a Terminal ID. UI does not choose Agent-vs-Shell kill commands. AppServer resolves the owning runtime, records intent before IO and persists a replay-safe receipt; an old receipt cannot close a resumed invocation. UI removes the tab only on `closed`, retaining it on error/uncertain delivery. Finished entries leave the Terminal inventory and tabs; saved history remains on the Card and in Overview. Show inactive reveals live idle Agents, not exited processes.

`TerminalEntry` projection and the native viewer name input-gating reasons Connecting / Disconnected / Closing. The internal `read_only` flag still rejects text/key/paste; it is no longer shown as a generic state. Static Gallery content is labeled Preview. The native stream still supplies precise connection-error details and retains immutable binding identity.

Isolated real-PTY acceptance at `/private/tmp/cb-close-000lb28a/close-evidence.json` passed manual and Agent termination, neighboring-Shell isolation, provider-history Resume, duplicate-close protection after Resume and unavailable-Core/explicit-retry behavior. These are deterministic Agent fixtures, not model invocations.


Final checks for this lifecycle update: 108 workspace Rust tests passed, including close receipt replay against a changed binding, Agent history/no-completion semantics, replaced-Core protection, explicit input-state projection and native frame/input gating. Application and Gallery packaging passed. A fresh cloned native acceptance window could not be inspected because computer-use access to Canban Acceptance was not approved; click-level validation of this update is not claimed. The real-PTY acceptance above exercised the production AppServer API directly.


### Desktop utilities (2026-11-09)

`ui-kit/tokens.rs` owns reactive semantic palettes; `chrome::utility_button` owns compact selected/disabled/hover/focus/accessibility states. `ui/commands.rs` supplies the shared key/help catalog. `ui/utilities.rs` and its panel module compose machine-scoped Usage, countdown, power/theme/help actions and one controlled popup, with generation-fenced async reads/writes. `ui/frame.rs` composes the shared bottom bar; its border excludes the navigation rail. `ui/github.rs` owns the retained target-Project Card composer and source-query invalidation. Native Terminal keeps its runtime identity while applying new foreground/background/ANSI/cursor colors.

Gallery workspace scenes now use the exact production frame, including utilities/overlays, with **Usage fixtures**, **No signed-in providers**, and full/720-point viewport controls. See [implementation and actual acceptance](desktop-utilities.md).

## Compact Card composition (2026-11-09)

`card/` separates fixed header, scrolling task/context/details, Timeline projection and fixed composer. `components/timeline_item.rs` owns inline file-backed Markdown replies/reports; `profiles.rs` owns the shared explicit-recipient menu/projection; `terminals.rs` owns Card-context navigation and explicit Agent selection. See [contracts and native evidence](card-conversation.md). Gallery **Card conversation** and **Append reply** cover long histories, narrow layouts and reading-position behavior through production components.

`card_actions.rs` shares lifecycle intents between the Card header, Board right-click menu and Archive rows. Local Cards use Archive/Restore; GitHub source Cards additionally retain reversible Remove from Board. These actions, archive failures and restoration use existing UI Kit chrome/buttons/panels. Gallery **Archived Cards** covers archived Project groups and excludes hidden legacy removed fixtures. The compact bottom bar omits the redundant Usage/machine prefix.

## Terminal interaction and navigation (2026-11-09)

The shared icon set includes Folder for grouping. Terminal rows use indented guides and a top selected Show/Hide inactive control; finished entries leave both tree and tabs. navigation.rs owns the reusable global chrome controls/history and keys across the regular and split Terminal frames. Native terminal input buffering, independent send/receive and dirty-row drawing live in crates/terminal; framed input activity belongs to Herdr transport/AppServer relay. [Contracts and verification](terminal-interaction.md) cover production/Gallery states.

## Named theme choice

`ui-kit/theme_choice.rs` composes an NSH button with a name, fixed source swatches, reactive selected border/check, hover and keyboard focus. It takes a static `canban-themes::Theme`, saved-selection and disabled readers, a hover-preview intent and a press handler; it owns no persistence or product state. A wrapper receives native mouse-enter/leave events while the inner NSH button retains click/keyboard/disabled behavior. `ui/utilities/panel.rs` composes two columns in the existing scrollable popup. The presenter holds ephemeral preview separately from saved preferences; only confirmation saves through AppServer. All screens use fixed-dark reactive semantic tokens; Terminal ANSI colors come directly from the shared catalog. Gallery exercises this production picker through the bottom Theme button. Settings uses the shared selected icon action at the bottom of the navigation rail; it is absent from the utility bar.

Terminal list metadata is owned by `ui/terminals/tree.rs` and `labels.rs`: selected titles wrap, Provider/Profile names deduplicate, state remains textual, and Local/catalog machine names are inline. List rows and their Provider labels have no tooltip and expose no directory; compact tab Provider rendering remains a separate variant. Project folders and the existing branch chrome supply the other context. The production Gallery contains Local/remote, custom-name, long-title and unavailable states.

### Profile-only Card editing (2026-11-10)

`Draft`, Card/Profile models and creation/update payloads no longer expose Routing tags. The production New Card and Task Edit controls contain only their applicable content fields; Board badges and Quick Open indexing no longer consume tags. Workflow header tooltips show the stage Profile or global-default fallback. Existing Gallery New Card/Card conversation scenes cover this composition.

Task Edit's structural owner depends on a computed editing/expansion projection rather than the whole Draft. Text changes must preserve the native input identity, focus and selection. A continuous-keyboard failure was reproduced in Gallery and fixed at `ui/card/task.rs`; final Gallery/App checks typed and retained complete text before saving. See [selection and native acceptance](product/agent-profiles.md#profile-only-selection-acceptance-2026-11-10).

## Markdown conversation components (2026-11-10)

`ui-kit::reminder_count` renders one compact numeric badge with a supplied
pending-action tone; zero is absent. `canban-notifications` owns Card reminder
identity/count projections. Board tiles, Project switching and Overview share
those counts, while the Card reader owns event-triggered visible-read reporting
and a persisted-unread jump affordance. See [unified reminders](reminders.md).

Shared motion now lives in `ui-kit/src/motion.rs`, with a retained native
`reveal_split` in `split.rs`. `ui/workspace.rs` keeps Board mounted across Card
open/close, dismisses an open right-hand Card/New Card pane when non-interactive
Board space is clicked on the left, and scopes the outgoing Card identity
through animation completion.
Markdown input resizing and the shared modal adapters reuse this policy. The
Gallery's `CANBAN_GALLERY_SCENE=motion` covers interruption, switching, resizing,
reduced motion and Quick Open. See [native motion](ui-motion.md) for contracts
and dated verification.

`ui-kit/src/markdown.rs` owns the common Markdown reader and `MarkdownInput`: an NSH rope editor, stable source/preview lifetime, formatting/undo actions, group focus/hold state, clipboard bridge and scoped submission shortcut. It accepts text/signals and callbacks without product, storage or runtime dependencies. `ui/src/card/{task,composer,attachments,timeline}.rs` owns revision-aware editing, explicit Profile recipients, Card-managed resource uploads and the chronological event projection. Timeline rows retain their owners by event ID; an echoed report can change from narrative to a compact event without repeating its body.

The Gallery's **Card conversation**, **Append reply** and **Reply in 5s** scenes exercise the production Card/editor composition, including rich Markdown and long conversation history. Native application acceptance additionally checks persisted operations against an isolated AppServer/Core. See [Card conversation](card-conversation.md) for the complete interaction contract and dated evidence.

## Archive tree and local search (2026-11-10)

`ui/archive.rs` composes the global archived-conversation browser; `archive/model.rs` owns grouping, metadata matching and ordering. The controlled UI Kit `tree_group` accepts title/count readers, expansion state, a toggle intent and children. It uses NSH disclosure/reveal behavior, keyboard activation, expanded accessibility state, indentation and a guide; hidden descendants do not accept pointer input or appear in accessibility navigation. Generic shared `search::score` preserves Quick Open matching, while `input::debounced` owns a trailing-edge, scope-cancelled timeout without replacing the native text field. Product-specific state and RPC remain outside UI Kit.

Gallery **Archived Cards**, or `CANBAN_GALLERY_SCENE=archive mise run gallery`, covers multiple Projects, an active-only Project, Unicode/long titles, Agent metadata and hidden legacy removed records, including a removed record with an archive timestamp. See [Archive contract and verification](archive.md).


## Global update and recovery components (2026-11-10)

`ui/updates.rs` owns the bottom-rail highlighted Update action, reactive progress/retry state, Settings check/version feedback and upgrade-session capture/restore. It composes UI Kit chrome/actions/notices and the shared update SVG; release discovery and install logic stay in `canban-updater`/AppServer. The update action is global across routes, disables duplicate submission and sends the exact offered version with the initiating packaged UI identity.

The UI saves root-owned Card drafts, pasted attachment bytes, current location, Terminal tabs and Archive query before acknowledging handoff. Restoration waits for the first applied snapshot so Card/Terminal identities resolve correctly; incomplete uploads become explicit retry states. This planned-update capture complements the durable editor draft writer described below; transient upload bytes stay out of SQLite and are carried separately in the upgrade session. Candidate readiness is acknowledged only after snapshot application and a rendered frame.

The production `Recovery` screen is a standalone host selected before AppClient startup when the install journal requires manual repair. Its inputs are home/error text; it displays the log location and Open update folder/Quit without opening SQLite. The Gallery reuses this exact screen with `CANBAN_GALLERY_SCENE=update-recovery`; Gallery Quit returns to its fixture shell. `CANBAN_GALLERY_SCENE=updates` opens pending-update fixtures; the full Gallery also exposes Update available/Update failed controls.

Native checks covered pending/progress presentation, recovery layout, real packaged Recovery exit with invalid SQLite bytes unchanged, and a packaged sidebar upgrade to `20261110-4` restoring an open Card/unsent comment. See [evidence and limitations](updates.md#executed-acceptance-2026-11-10).

## Unified Card editing and durable drafts (2026-11-10)

`ui/src/editor.rs` shares Markdown source, preview, reference completion and draft
feedback between New Card and task editing. `ui/src/references.rs` supplies local
Card/Profile candidates and bounded GitHub Issue/PR lookups. UI Kit owns the
`Menlo, PingFang SC` editor font cascade, Enter/Tab completion acceptance,
Enter/Shift+Enter newlines and scoped Command+Enter submission. New Card binds
guarded Escape dismissal, and the right-pane split retains its editor while expanded.

`ui/src/drafts.rs` owns window projections, `drafts/list.rs` the searchable Drafts
screen, and `drafts/persistence.rs` one coalescing background writer. AppServer
`drafts.rs` owns revision-checked SQLite persistence outside Card/Timeline state.
The Gallery's `CANBAN_GALLERY_SCENE=editor` imports the production New Card flow.
The `board-drafts` scene exercises counted cross-Project drafts and Update above
bottom-pinned Settings; `board-drafts-empty` exercises the count-free entry and
empty list. Both use the production frame, Board and Drafts navigation.
See [Card editor](card-editor.md) for native verification and limits.

## Markdown reference links (2026-11-10)

`ui-kit/src/reference_link.rs` owns the generic inline link's style, focus,
350 ms rich tooltip and navigation callback. It receives preview content and
callbacks without product or network dependencies. `ui/src/links.rs` owns
Card/Profile/attachment/web routing, local previews and bounded GitHub detail
lookup; `links/document.rs` decorates the parsed Markdown AST outside code.
The root-installed NSH Markdown plugin returns inline Text spans, retaining the
paragraph baseline and hit targets. It is installed in both App and Gallery roots.

Task, editor Preview and Timeline narrative readers use this common component.
`CANBAN_GALLERY_SCENE=references mise run gallery` supplies 65 Cards, body/comment
Markdown and local/Profile/web/GitHub links. Native App acceptance additionally
verifies exact saved completion text and real default-browser navigation; see
[Card editor](card-editor.md#reference-follow-up-acceptance-2026-11-10).

## Workflow authoring components

`ui::workflows` owns the Project source editor, SQL history/restore-as-draft and direct **Edit with agent** launch (`edit_agent`). Clicking **Edit with agent** in the top-right header (or invoking `Edit Workflows with Default Agent` from the Command Palette) directly dispatches `configuration.open` using the configured default Agent (`sessions::default_profile`, respecting Project overrides) with an English-only Project-wide Prompt (`workflow.prompt`), opening or continuing/resuming the configuration Terminal without an intermediate Agent selection modal. Native NSH editors preserve text selection and local undo; UI state retains per-Project drafts across navigation. Source validation/conflict errors preserve the draft. Successful publication quietly clears the unsaved marker; active-version numbers stay out of the main editor. New workflow sits below the source list, and Save/Command+S share eligibility and publication. NSH YAML highlighting, two-space indentation, line numbers and soft wrapping are enabled. History uses a native modal layer with focused-editor Escape dismissal and caret restoration; `agent_panel` remains for read-only Prompt inspection in the `workflow-agent` Gallery scene (showing the fixed `@default` badge without a profile selector). `workflow-agent`, `workflows`, `workflows-conflict` and `column-automations` Gallery scenes compose production components with deterministic fixtures.

Board column headers render a read-only automation pill (`ui::workflows::column_automation_pill`, `Icon::Zap`) derived from `workflow-engine::source::column_automation` when the column has active automatic steps (`Auto develop`, `Auto merge`, `Auto plan`, `Auto start`, `Auto @<profile>`, plus `+N` when multiple workflows target the column). Hovering shows a styled `tooltip_box` summarizing the active workflow file and step chain; clicking performs no navigation, and manual/approval-only columns render nothing. Configuration Agent entries are grouped under Project / Configuration in the existing Terminal inventory and offer Back to Workflows.

## Card conversation refinement (2026-11-11)

`card/header.rs` owns local/Issue/PR identity; `github/references.rs` owns the
saved-content reference projection. `components/timeline_item.rs` accepts an
Event and narrative classification, rendering shared author/time/Markdown
message chrome or a compact system row. It owns local full-body expansion and
report loading; navigation retains explicit request identity. The Card Timeline
owns adjacent routine-event grouping, read coordinates and execution disclosure.
`MarkdownInput` keeps nonempty compact drafts expanded on blur for every consumer.
## UX refinement: Unified 42pt Chrome, Global Activity Popover, Card + Live Terminal & Managed Worktrees (2026-11-13)

- `ui/frame.rs`: Owns the unified 42-point window chrome across all non-Terminal routes (matching exact traffic-light + `<` / `>` Back/Forward geometry with `terminals.rs`), mounting route-specific header slots (`Board` Project picker + cross-Project activity chips + right-aligned Project actions, `Card`, `Diff & Review`, `GitHub`, `Settings`, and sub-pages) plus the persistent far-right `overview::trigger_pill` and right-anchored `overview::popover`.
- `ui/rail.rs`: 56-point icon rail (`Quick Open`, `Board`, `Terminals`, `Diff & Review`, `GitHub`, conditional `Update`, and bottom `Settings`), omitting the superseded standalone `Overview` route.
- `ui/overview.rs`: Composes the persistent top-right status trigger (`⚡ N · ● active/max`, `Command+I`) and right-anchored Activity & Notifications popover (`Needs you`, `Updates` with inline `Read` / `Read all`, `Running` / `Queued` / `Idle` / `Closed`, and 1-step **Card & Terminal →** navigation) using `ui-kit::tone_icon_badge` (28×28 semantic icon tiles) and an event-first hierarchy (Line 1 semantic headline + cleaned summary, Line 2 `#number title · project · @profile`), while retaining pure production row helpers for Gallery regression scenes.
- `ui/card/terminal.rs`, `ui/card.rs`, `ui/workspace.rs`: Side-by-side left-right `Card + Live Terminal` split (`Command+J` / `>_ Terminal`), single-attachment `canban/terminal` viewer mounted exclusively on `"card"` route, non-destructive viewer detach on hide/close, explicit Markdown editor vs PTY keyboard focus ownership, Agent profile selector, `[Interrupt]`, and `[Pop out to Terminals ↗]`.
- `ui/terminals/worktrees.rs`, `ui/terminals/tree.rs`, `ui/terminals/tabs.rs`: `New Managed Branch Worktree…` creation modal, persistent `ManagedWorktree` tree nodes retained across `Command+W` tab closes, quick reopen (`Shell` / `@Agent`), `Open in Diff & Review`, `Promote to Card…`, and guarded `Delete Worktree…`.
- `ui/commands.rs`, `ui/quick_open.rs`: Full Command Catalog (`>`) parity covering global chrome, Notifications popover, Card operations, Terminal & Worktree lifecycle, utilities, config reload, and updates.
- `ui-kit/settings.rs`, `ui/settings.rs`: Settings middle jump column (`INTERFACE`, `EXECUTION`, `SYSTEM`, and a separated `Acknowledgements` entry; horizontal jump strip at `< 950` points) paired with a single outer content scroll, scroll-spy + click-to-jump target pinning, `Open Acknowledgements & Licenses` command (`go-acknowledgements`), and the bottom `Acknowledgements & Licenses` section (`Herdr`, `sheprd`, `NSH`, `Ghostty`, asset credits, and collapsible bundled license/notice viewer). Gallery `settings`, `acknowledgements` and `controls` scenes exercise these production components.
