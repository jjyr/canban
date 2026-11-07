# UI component inventory

Status: implementation inventory, updated 2026-11-07. These modules are used by the native application; the Gallery imports the same implementations.

| Component/layer | Module | NSH reuse and contract | Verification |
| --- | --- | --- | --- |
| Semantic tokens | crates/ui-kit/src/tokens.rs | Shared adaptive surfaces/text/accent/borders and sizing | Native light/dark Gallery and Board inspected |
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

crates/ui/src/sessions.rs owns the shared Card profile entries and Agent terminal screen. Input is the selected request/attempt and profile registry; actions open/resume, interrupt, end or explicitly start fresh context. Navigation detaches only the viewer. crates/terminal owns the native canban/terminal component with immutable binding (config, core ID, terminal address), read_only input gating and an independent stream worker. It has no Card/workflow/storage dependency. The Gallery uses this production native component with a deterministic VT fixture. Read-only is enabled during completed/closing/closed states; uncertain-close errors remain inspectable.

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
| `ui/board.rs` | Project snapshot, selected Card, filter, stage scroll store; open/move intents | All/attention/running/review filters, horizontal overflow and independent column scroll |
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

`ui/overview.rs` composes the global grouped list, stable Card/profile rows, filters, Closed/history disclosure, context inspector and explicit destination actions. Inputs are the shared typed pure projection plus retained UI selection/filter/scroll state; selection has no execution side effects. Inspector actions remain outside the scrolling assignment/history. Narrow layouts show the same context with an explicit return. Preview requests are asynchronous and generation/identity fenced. The Gallery Overview scene supplies every group, independent reasons, multiple Projects and request-less recovery. `ui-kit::read_text` reuses NSH selectable document behavior for plain Agent/runtime text without interpreting markup.


### Compact navigation and search

`ui-kit::Icon` and `icon_action` define original consistent line icons and 40-point accessible navigation targets. `ui/rail.rs` composes the 56-point product rail, global attention badge and Project picker. `ui-kit::SearchDialog` takes prepared `SearchChoice` values and reuses NSH Dialog/Command for the popup and candidate list; product targets and fuzzy ranking stay in `ui/quick_open`. Command+O, immediate Escape, outside dismissal, candidate highlight and exact read-only navigation share the same production components in the Gallery. See [interaction and acceptance](quick-open-and-navigation.md).
