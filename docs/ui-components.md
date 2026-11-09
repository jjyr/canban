# UI component inventory

Status: implementation inventory, updated 2026-11-08. These modules are used by the native application; the Gallery imports the same implementations.

| Component/layer | Module | NSH reuse and contract | Verification |
| --- | --- | --- | --- |
| Semantic tokens | crates/ui-kit/src/tokens.rs | Shared adaptive surfaces/text/accent/borders and sizing | Native light/dark Gallery and Board inspected |
| Scroll indicators | crates/ui-kit/src/scrollbar.rs | App-wide NSH overlay scrollbars: 6-point rounded muted thumb, transparent 12-point track, reveal on scroll/track hover, fade after idle; colors follow theme preview and selection. NSH owns metrics, dragging, track clicks and reduced motion. No layout width is reserved. Native Terminal rendering remains separate. | Three overflowing production Board columns in Gallery; isolated actual App scroll, theme switch and Card navigation |
| Button | crates/ui-kit/src/button.rs | nsh-base Button; typed variants, centralized hover/pressed/focus/disabled styles | Native mouse, Tab/Space and disabled-skip checks |
| Text field | crates/ui-kit/src/input.rs | Controlled NSH Input; common geometry/focus/text | Multiline Unicode paste/edit/save, live Markdown preview and retained draft through state refresh |
| Checkbox | crates/ui-kit/src/checkbox.rs | Controlled nsh-base Checkbox; common checked/focus presentation | Used by per-Card Note selection; snapshot semantics have Rust regression coverage |
| Panels/text/badge | crates/ui-kit/src/surface.rs | Shared NSH layout/text composition | Native themes and real Card composition inspected |
| Card tile | crates/ui/src/components/card_tile.rs | Product projection, press-to-open, dedicated pan handle and cancellation | Native Backlog -> Develop drag invoked real scheduling |
| Timeline item | crates/ui/src/components/timeline_item.rs | Comment Markdown, event provenance, report/output intents | Production and Gallery fixtures; explicit reports through real Herdr/CLI |
| Agent profile picker / registry | crates/ui/src/profiles.rs | Shared configured profile projection; inserts @profile into a retained composer draft; Settings shows role instructions | Native @reviewer selection/submission and fixture completion verified; Board labels reflect template references |
| Agent run/output panel | crates/ui/src/components.rs | Request/attempt view, retry and read-only output | Request history and retained output; interactive Terminal lives in sessions.rs |
| Context picker | crates/ui/src/components/context_picker.rs | Per-Card Project Note selection via shared Checkbox | Immutable selected-context regression; native checkbox toggle and saved revision verified |
| Presenters | crates/ui/src/presenter.rs | Async AppServer operations, event-driven refresh, draft preservation and resource generation checks | Live state updates, native saves and image-paste refresh |
| Product screens | crates/ui/src/screens.rs | Compose production components; application navigation only | Board, Card and Notes native checks |
| Standalone Gallery | examples/gallery/main.rs and fixtures.rs | Imports production components from canban-ui; local fixture state without AppServer/data/Core | Run mise run gallery for component checks |

Components use their current typed Rust inputs/events; longer-form component API docs can grow with the public surface. Menus/dialogs/rich-editor/virtual-list wrappers should be added when a concrete feature needs them, using NSH facilities first. Do not create parallel screen copies or claim unimplemented Gallery scenarios as tested. The [design-system conventions](ui-architecture.md) govern subsequent changes.

## Interactive Agent components (2026-11-07)

crates/ui/src/sessions.rs owns the explicit Start/Resume intent; Card entries now navigate through the compact header and the Terminal Choose Agent control. The dedicated Terminal page/tree/viewer now live in crates/ui/src/terminals.rs and terminals/; the earlier Card/Terminal focused split is superseded. Input is the selected request/attempt and profile registry; actions open/resume, interrupt, end or explicitly start fresh context. Navigation detaches only the viewer. crates/terminal owns the native canban/terminal component with immutable binding (config, core ID, terminal address), read_only input gating and an independent stream worker. It has no Card/workflow/storage dependency. The Gallery uses this production native component with a deterministic VT fixture. Read-only is enabled during completed/closing/closed states; uncertain-close errors remain inspectable.

Native terminal regression scenarios also cover hidden cursor/null cursor metadata, wide-glyph background ordering, clipping to native bounds and scrolling a static Gallery fixture into view. The component redraws on native visibility/window-position changes as well as VT revision changes.

## Review delivery component (2026-11-07)

`crates/ui/src/review.rs` owns the Review route and reusable `diff_panel`. Input is a typed immutable ReviewPreview containing Card/revision, target/heads, files, patch, token and blockers. The same diff presentation is rendered in the native Gallery. Native actions refresh the diff, return to Develop, or explicitly accept all reviewed changes with a commit message. Async diff responses are fenced by generation, Card and target; changed Card revisions/targets disable acceptance, and the server revalidates content/heads. Returned failure/unknown receipts are visible and clear the preview. Reports and @Agent discussion reuse the Card components. No duplicate UI Kit controls or Shear editor are introduced.

Terminal reconnection keeps the immutable Core/terminal identity, waits for native layout before initial attach, reapplies current size/focus after handshake and drops disconnected/uncertain input. Precise wheel deltas accumulate and are converted to libghostty's negative-up direction. The 12,000-line Model regression and native upward/downward scroll checks cover the shared behavior.

## Standalone Gallery ownership

The development Gallery and its sample records/VT data live exclusively in `examples/gallery`. `canban-ui` exports reusable production modules as a library; its small binary calls the desktop host. Main-app navigation contains no Gallery entry/route, and its startup does not select a development Gallery mode. `mise run gallery` builds a separate Cargo example and `.local/CanbanGallery.app`, with its own bundle ID and no bundled Canban CLI. Component development and regression scenes are checked there; see [example instructions](../examples/README.md).

The native Terminal's optional `content` property replaces a static VT preview when there is no runtime binding; repeated identical content is idempotent. Live bindings still use Herdr. Sample VT bytes are supplied by the example rather than embedded as a production `fixture` branch.


## Kanban workspace and Notes components (2026-11-07)

| Owner | Input/state and interaction contract | Composition and verification |
| --- | --- | --- |
| `ui-kit/navigation.rs` | Label, selected predicate, press intent; selected/hover/focus remain separate | Sidebar and Note list share selection styling |
| `ui-kit/status.rs` | Semantic `Tone`, chip/notice text; state remains textual as well as colored | Runtime/attention/result feedback in Board, Card and Notes |
| `ui-kit/split.rs` | Left/right elements, retained width signal, minimum widths; NSH owns resizing/focus/accessibility | Board/Card, focused work and Notes layouts |
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

`ui/overview.rs` composes the global grouped list, stable Card/profile rows, nonempty status groups, Closed/history disclosure, context inspector and explicit destination actions. Inputs are the shared typed pure projection plus retained UI selection/scroll state; selection has no execution side effects. Inspector actions remain outside the scrolling assignment/history. Narrow layouts show the same context with an explicit return. Preview requests are asynchronous and generation/identity fenced. The Gallery Overview scene supplies every group, independent reasons, multiple Projects and request-less recovery. `ui-kit::read_text` reuses NSH selectable document behavior for plain Agent/runtime text without interpreting markup.


### Compact navigation and search

`ui-kit::Icon` and `icon_action` define vendored Lucide native-vector icons and 40-point accessible navigation targets. `ui/rail.rs` composes the 56-point product rail, global attention badge. The Board header owns the Project picker and New Project entry. `ui-kit::SearchDialog` takes prepared `SearchChoice` values (including a current-item marker), controlled placeholder and caller-composed footer and reuses NSH Dialog/Command for the popup and candidate list; product targets and fuzzy ranking stay in `ui/quick_open`. Command+O, immediate Escape, outside dismissal, candidate highlight and exact read-only navigation share the same production components in the Gallery. See [interaction and acceptance](quick-open-and-navigation.md).


### Project entry and Settings components (2026-11-08)

`ui-kit/entry_card.rs` owns the single accessible clickable entry; `directory_field.rs` owns the editable path/native folder sheet with cancellation and disposed-owner guards; `settings.rs` owns generic section/field composition. Product UI owns Project name suggestions and creation. One screen scroll contains Agent scheduling, Agent profiles, Workflow, Herdr and Storage; the scheduler component uses the same section/field geometry. `profiles::registry_items` supplies content without nested panel styling. The Gallery includes Empty Board, Settings, all-state grouped Overview and entry/directory controls at a 720-point constrained width. See [contracts and native verification](workspace-entry-and-settings.md).

The simplified creation presenter lives in `ui/projects.rs`: Directory precedes Name, basename suggestions stop after an explicit name edit, and NSH Collapsible keeps Execution machine secondary. `ui-kit::directory_input` owns the common path/Choose row geometry; `directory_field` supplies its native picker, while `ui/machines::directory_picker` supplies the selected host's server browser. New Project and Work location share that path component, with one chooser action each. Gallery has an explicit New Project entry and machine fixtures for path clearing, custom-name preservation and collapsed remote summaries.


### GitHub Card and unified creation (2026-11-09)

`ui/github.rs` composes account/source Settings, the unified Local/GitHub form and source/PR sections from shared controls. Add Card opens that form directly. Query/result/selection projections isolate reactive updates so search responses do not replace the text input; type changes preserve separate drafts. Long remote descriptions live inside the Card's main scroll area. Card tiles use a compact GitHub mark and source-number tooltip. `ui-kit::action` now accepts reactive labels, and `Icon::GitHub` uses the official MIT-licensed Octicons mark with theme color. The Gallery includes the production GitHub Card scene. See [implementation and validation](github-integration.md); a fixture scene is not live GitHub authorization acceptance.

### Dedicated Terminal workspace (2026-11-08)

`ui/terminals/entries.rs` projects Agent conversations and durable manual Shells into one inventory. `tree.rs` and `tabs.rs` retain native pointer/focus targets across irrelevant runtime timestamp updates. `terminals.rs` owns a full-height resizable split: traffic lights/navigation over the left inventory, and numbered tabs/Card/branch over the native viewer. `ui/frame.rs` is the production frame shared with the standalone Gallery. UI Kit's `chrome_button` provides compact accessible icon controls; all secondary actions use native NSH menus anchored to their trigger or context-menu pointer. Native Terminal focus is explicit and suspended for search/rename; its accessibility value includes connection status and emits throttled change notifications. The permanent technical footer is removed; operation failures use dismissible notices. See [contracts and acceptance](terminal-workspace.md) for manual Shells, close-versus-end, exact Agent resume and validation limits.

Window-owned commands use `Ui::spawn` so runtime refresh, button replacement and navigation cannot cancel publication of a completed receipt or leave `pending` stuck. Page-specific observations remain scoped/cancellable; command-driven navigation and local editors retain source fences. A snapshot epoch rejects refreshes started before a newer command receipt. The native Terminal accepts UI-declared Command keys and suspends PTY input synchronously before the asynchronous UI focus transfer; search/rename/pending actions keep input gated until focus returns.

### Independent Terminal context (2026-11-08)

The production inventory groups entries by task Project (managed Agents), observed Project (independent Terminals), or Other. Card hints are chrome links, not nested inventory groups. `Entry` combines durable presentation identity, optional task association and ephemeral Herdr directory context; the native binding projection excludes directory hints so regrouping cannot remount the PTY. Context subscriptions do not change Board selection or tab order. Card return and rail navigation issue explicit focus tickets. Gallery Terminal fixtures include unassociated and inferred-Card independent Terminals alongside managed Agents.

### Terminal title presentation

`ui/terminals/labels.rs` is the shared product component for provider/Profile marks and bounded tab labels. `Entry` derives automatic titles and the selected Request stage. The inventory reserves its first-line stage and second-line status around a flexible title/Profile, while independent Terminals stay single-line without a type icon. Tab labels reserve stage/Profile and a separate close button. Native tooltips/accessibility text retain full source values. Long branch/directory labels use middle ellipsis; their hover retains the complete value. The Gallery has Codex/Claude, custom providers, Unicode/long Card titles, multiple Profiles and independent numbered entries. UI Kit only supplies reusable provider vector assets and generic rendering.


### Quiet Board Cards and direct type selection (2026-11-09)

`ui-kit/segmented.rs` provides direct single-choice buttons with shared adaptive tokens and separate selected, hover and keyboard-focus states. The New Card form uses Local / GitHub Issue segments, including the official GitHub mark, instead of a type menu. Drafts remain independent across type switches.

Board headers show only the workflow stage name and count. Default Agent routing is available in the header tooltip, so Develop / Review stages cannot be mistaken for developer / reviewer Profiles. Custom workflow stage names remain unchanged. Card tiles omit repeated stage labels, drag instructions, empty metadata rows and inactive/completed/queued execution summaries. A compact top-right grip retains the existing drag behavior. Titles remain primary, excerpts are single-line, branch text is bounded with a full tooltip, and tags wrap. `components/activity.rs::card_activity` shares active provider/Profile identity and actionable human-gate signals between tiles and Card details; provider detection is shared with Terminal rows through `profiles::provider`.

The standalone Gallery adds Card states and New Card scenes, including queued, executing, human-gated and long-title Cards at constrained width. Native acceptance at `/private/tmp/cb-native-o55cpz5o` verified small-grip Backlog → Develop drag with actual fixture scheduling, direct type switching with exact title/body retention, and Local Card creation. UI tests (12), targeted strict all-target Clippy, application/Gallery packaging and diff whitespace checks passed. These interaction fixtures do not invoke a model or use the user's live Core.


### Terminal close lifecycle and input state (2026-11-09)

X, Command+W and context-menu Close terminal invoke one AppServer `terminal.close` operation with a Terminal ID. UI does not choose Agent-vs-Shell kill commands. AppServer resolves the owning runtime, records intent before IO and persists a replay-safe receipt; an old receipt cannot close a resumed invocation. UI removes the tab only on `closed`, retaining it on error/uncertain delivery. Closed history remains in inventory.

`TerminalEntry` projection and the native viewer name input-gating reasons Connecting / Disconnected / Closing. The internal `read_only` flag still rejects text/key/paste; it is no longer shown as a generic state. Static Gallery content is labeled Preview. The native stream still supplies precise connection-error details and retains immutable binding identity.

Isolated real-PTY acceptance at `/private/tmp/cb-close-000lb28a/close-evidence.json` passed manual and Agent termination, neighboring-Shell isolation, provider-history Resume, duplicate-close protection after Resume and unavailable-Core/explicit-retry behavior. These are deterministic Agent fixtures, not model invocations.


Final checks for this lifecycle update: 108 workspace Rust tests passed, including close receipt replay against a changed binding, Agent history/no-completion semantics, replaced-Core protection, explicit input-state projection and native frame/input gating. Application and Gallery packaging passed. A fresh cloned native acceptance window could not be inspected because computer-use access to Canban Acceptance was not approved; click-level validation of this update is not claimed. The real-PTY acceptance above exercised the production AppServer API directly.


### Desktop utilities (2026-11-09)

`ui-kit/tokens.rs` owns reactive semantic palettes; `chrome::utility_button` owns compact selected/disabled/hover/focus/accessibility states. `ui/commands.rs` supplies the shared key/help catalog. `ui/utilities.rs` and its panel module compose machine-scoped Usage, countdown, power/theme/help actions and one controlled popup, with generation-fenced async reads/writes. `ui/frame.rs` composes the shared bottom bar; its border excludes the navigation rail. `ui/github.rs` owns the retained target-Project Card composer and source-query invalidation. Native Terminal keeps its runtime identity while applying new foreground/background/ANSI/cursor colors.

Gallery workspace scenes now use the exact production frame, including utilities/overlays, with **Usage fixtures**, **No signed-in providers**, and full/720-point viewport controls. See [implementation and actual acceptance](desktop-utilities.md).

## Compact Card composition (2026-11-09)

`card/` separates fixed header, scrolling task/context/details, Timeline projection and fixed composer. `components/timeline_item.rs` owns inline file-backed Markdown replies/reports; `profiles.rs` owns the shared explicit-recipient menu/projection; `terminals.rs` owns Card-context navigation and explicit Agent selection. See [contracts and native evidence](card-conversation.md). Gallery **Card conversation** and **Append reply** cover long histories, narrow layouts and reading-position behavior through production components.

`card_actions.rs` shares lifecycle intents between the Card header, Board right-click menu and Archive recovery rows. Direct Archive/Restore, reversible local Delete/Remove, archive failures and recovery use existing UI Kit chrome/buttons/panels. Gallery **Archived / deleted** covers both recovery groups. The compact bottom bar omits the redundant Usage/machine prefix.

## Terminal interaction and navigation (2026-11-09)

The shared icon set includes Folder for grouping. Terminal rows use indented guides and a top selected Show/Hide inactive control; finished entries leave both tree and tabs. navigation.rs owns the reusable global chrome controls/history and keys across the regular and split Terminal frames. Native terminal input buffering, independent send/receive and dirty-row drawing live in crates/terminal; framed input activity belongs to Herdr transport/AppServer relay. [Contracts and verification](terminal-interaction.md) cover production/Gallery states.

## Named theme choice

`ui-kit/theme_choice.rs` composes an NSH button with a name, fixed source swatches, reactive selected border/check, hover and keyboard focus. It takes a static `canban-themes::Theme`, saved-selection and disabled readers, a hover-preview intent and a press handler; it owns no persistence or product state. A wrapper receives native mouse-enter/leave events while the inner NSH button retains click/keyboard/disabled behavior. `ui/utilities/panel.rs` composes two columns in the existing scrollable popup. The presenter holds ephemeral preview separately from saved preferences; only confirmation saves through AppServer. All screens use fixed-dark reactive semantic tokens; Terminal ANSI colors come directly from the shared catalog. Gallery exercises this production picker through the bottom Theme button. Settings is a selected utility button in the bottom bar, not a rail item.
