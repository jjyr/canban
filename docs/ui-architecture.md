# UI architecture and design system

Status: user-approved architecture and development direction, 2026-11-06, now applied in the first native MVP. The approved UI Kit/product UI separation, suitable NSH reuse, component contracts, native gallery and component-level regression workflow guide implementation. Further components and visual refinements can extend the initial system.

Current contract, reconciled 2026-11-17: the native UI, shared UI Kit and production-component Gallery exist. This document governs component ownership, interaction design and UI data/resource costs; [implementation](implementation.md), the [component inventory](ui-components.md) and feature documents identify actual behavior and acceptance. Feasibility/research examples below do not establish runtime coverage.

## Architecture

```text
canban-ui
  Screens: Board / Card (+ side-by-side Terminal) / Terminals / Git / GitHub / Insights / Settings
  Global overlays: Quick Open & Command Palette / Right-side Activity & Notifications popover
       |
  Product components: CardTile / StageColumn / TimelineItem / EmbeddedTerminal
                      AgentRunPanel / MentionComposer / ReportViewer / ChromeSlots
       |
       +----------------------------> canban-ui-kit
       |                                semantic tokens / typography / icons
       |                                styled controls / feedback / layouts
       |                                shared UI interaction conventions
       |                                        |
  Presenters / UI state                         v
       |                                    nsh-base
       v                                controls and interaction behavior
  AppServer                                     |
                                                v
                                            nsh-core
                                      elements / runtime / platform
```

Screens compose components and connect user intent to application operations. Product components own reusable Canban presentation; they may accept narrow UI models and emit intents. Presenters own screen loading/subscriptions, selection and operation feedback. AppServer owns saved business state and workflow/scheduling policy. UI Kit has no AppServer, database, Herdr, Card ID, or workflow dependency.

Use package canban-ui-kit in crates/ui-kit, separate from canban-ui in crates/ui. Use ordinary files/modules for individual controls and product components; do not create a crate for every widget. Example: ui-kit/src/button.rs versus ui/src/components/card_tile.rs. These boundaries are implemented by the workspace; see the [crate plan](architecture-crates-and-timers.md).

## Product hierarchy and interaction principles

```text
Unified 42-point Window Chrome (all pages)
  +-- Left: traffic-light clearance + stable Back/Forward (< >) + page context/breadcrumbs
  +-- Center: window drag region
  +-- Right: page primary actions + global Activity & Notifications trigger [⚡ N · ● running]
             |
             +--> Right-side Activity & Notifications Popover (Command+I, global across pages)
                    Needs you / Updates / Running & Queued -> inline read/merge or 1-step jump

Board (Project work and workflow)
  +-- Card workspace: left-right side-by-side split
  |     Left: task/context + scrolling Timeline + approval + composer
  |     Right (toggleable): live native Terminal for the Card's Agent (or [Pop out ↗] to Terminals)
  +-- Git: report, ad-hoc Git compare, Diff and explicit delivery/rework

Terminals (direct interaction & standalone Git workspaces)
  +-- managed Card/profile Agents + standalone Managed Branch Worktrees + independent manual Shells
```

Board is the management center. Global cross-Project execution and human intervention live in the persistent top-right **Activity & Notifications** popover (`Command+I`) rather than a separate Left Rail page, eliminating two-hop "open Overview → inspect → jump again" detours. Card is a self-contained problem-solving workspace: users can read context, comment with `@agent`, and toggle a **left-right side-by-side Live Terminal** directly beside the Card without losing visual context, while `Terminals` remains available for full-screen multi-tab PTY management, manual Shells, and **Canban-managed standalone Git branch worktrees** (`New Branch Worktree…`, retained across tab closes until explicitly promoted to a Card or deleted). Project creation/selection and primary page actions belong in the unified 42-point window chrome; the 56-point rail stays compact (`Quick Open`, `Board`, `Terminals`, `Git`, `GitHub`, `Insights`, and bottom `Settings` + conditional `Update`). See [Card composition](card-conversation.md), [Global Activity & Notifications](product/overview.md), and [Terminal workspace](terminal-workspace.md) for exact layouts.

- **Unified 42-point window chrome:** Every screen shares one 42-point top chrome bar. Native traffic-light clearance and `<` / `>` Back/Forward buttons occupy identical horizontal coordinates across all routes (including `Terminals`) so navigation controls never shift when switching pages. Page identity, breadcrumbs, sub-navigation tabs, and primary page actions render in the chrome bar rather than stacking duplicate large page headers inside content scroll regions, and the global Activity & Notifications pill anchors to the far right on every route.
- Put task identity and current actionable state first. Omit repeated drag instructions, empty execution labels, duplicate stage labels and permanent technical captions. Keep failure and human-action feedback discoverable; quiet presentation must not conceal a failed Agent.
- Display frequently needed context inline and disclose secondary details on demand. Terminal list rows show title/stage, Provider/Profile, state and machine without metadata popovers or current-directory text. Branch full-value tooltips and secondary menus remain useful. This is not a blanket ban on hover: Theme intentionally previews on hover, then saves on confirmation.
- Reserve essential controls before allocating flexible title space. Test long names, mixed-language titles, narrow windows, overflow and empty/error/pending states. Truncate compact rows; offer full text through selection, expansion or an appropriate tooltip according to the component contract.
- **One operation path & Command Catalog parity:** Every user-executable action across global chrome, Board, Card, Diff & Review, Terminals, Managed Worktrees, GitHub, and Settings must register in the searchable Command Palette (`Command+/`, `/` outside text inputs, or `Command+O` with `>`) and dispatch the exact same operation intent and eligibility/pending checks as its button, shortcut, or context menu. New Card Create and comment Send share Command+Enter as a convention, with their own scoped validity/pending checks. Escape/outside dismissal and focus return follow the owning overlay's contract.
- **Single active native PTY attachment:** A Herdr PTY stream may be mounted in at most one active `canban/terminal` viewer at any time (either inside the Card's side-by-side split on the Card workspace or inside `Terminals`, never double-mounted simultaneously). Hiding the Card Terminal pane or navigating away detaches only the UI viewer without terminating the underlying PTY; explicit focus ownership keeps Card Markdown editing (`composer` / `task`) separate from embedded PTY key capture.
- Preserve drafts, selected context, scroll and focus across navigation and background refresh. Do not remount a native Terminal because unrelated metadata changed. Async results must remain fenced to the resource/binding and current generation.

## NSH reuse boundary

The original NSH research inspected revision b7a69186d3c20a59690f2ac834dc527bee7f8438; this is a historical source reference, not the current dependency pin. See [implementation](implementation.md) and Cargo.lock for the shipped revision. Its [component documentation](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/docs/dev/components.md) explicitly separates nsh-base behavior from the app-owned styled layer. Base controls handle behavior/focus/keyboard/accessibility and accept application styling; they are generally unstyled, with documented reading/editor/diff exceptions. Rust apps have direct builders. Canban needs its own consistent visual layer rather than assuming NSH supplies a finished product design system.

For each needed component, record one of: reuse NSH directly; compose/style NSH in UI Kit; implement a missing product behavior; or propose an NSH fix/extension. Start with behavior reuse, and preserve existing focus/accessibility semantics when composing controls. Do not wrap every layout element or replace working NSH keyboard handling merely for naming consistency.

| Need | Initial reuse route | What Canban must establish |
| --- | --- | --- |
| Buttons, input, selection, tabs | NSH controls + UI Kit styling | Variants, sizes, labels, focus and disabled/loading behavior |
| Menus, popovers, dialogs, tooltips | NSH overlay/control facilities | Consistent composition, dismissal, focus return and placement |
| Split panels, long lists | NSH resizable/list facilities | Desktop sizing, scrolling, virtualization and focus acceptance |
| Card/comments editing and reading | Evaluate Editor, RichEditor and TextView | Actual Rust API fit, Markdown/content model, IME, mentions, paste and selection |
| Board drag and drop | Reuse suitable NSH input/gesture primitives | Card movement, cancellation, nested scrolling, autoscroll, keyboard equivalent |
| Run output and reports | Shared product panels using reading primitives | Bounded loading, follow-output policy, file-backed content and archive loading |

Availability in source is not acceptance in Canban. RichEditor is not automatically a complete Markdown/mention composer; gesture primitives are not an already-validated Kanban board. Complex editing/drag/list components need a small native feasibility slice before committing the whole product to their APIs.

## Design system scope

Treat the design system as visual and behavioral contracts implemented by reusable components, demonstrated in a live gallery, and maintained by regression scenarios.

Semantic tokens cover surfaces, text emphasis, border, accent, focus and feedback tones; typography; spacing; control sizes; radii; icons; density; and motion preferences. Components consume those names. Canban uses fixed-dark native controls, defaults to Canban and offers 16 additional named Terminal palettes. Hover previews temporarily; activation confirms. There is no System/Light/Dark selector. The [desktop utilities contract](desktop-utilities.md) owns the catalog, persistence and preview rules; components consume its shared semantic tokens. Product screens should not invent independent button colors or input heights. Feature-specific layout is allowed within the common scale; a real new semantic need should extend the system explicitly.

Do not overload one color or status enum with stage, execution and attention. A Card may be in Review, have a queued execution, and need attention at the same time. Product presentation maps these distinct facts to shared badge/status visuals; the generic UI Kit does not learn Kanban workflow rules.

Define applicable states for each control: default, hover, pressed, keyboard focus, disabled, pending and error. Define composition behavior: focus entry/return, Escape and outside-click handling, shortcut scope, text selection, copy/paste, IME composition, overflow/truncation, minimum sizing and scroll ownership. Not every component needs every state, but omitted behavior should be intentional. Every clickable surface shows the pointing-hand cursor while enabled and the default arrow while disabled; use UI Kit `press_target()` for press targets, and add `cursor_pointer()` to custom pressable views (Board Cards, Terminal tabs, inline reference links, tree headers, Quick Open rows). Modal backdrop dismissal areas and text inputs keep their platform cursors.

Cross-cutting focus/overlay/command conventions belong in shared UI facilities on top of NSH, not independently in each screen. This need not become a new framework; reuse NSH's mechanisms and centralize only the app policy/adaptation required. For optional WebViews, map the same semantic tokens and explicit focus/keyboard/bridge contracts into the embedded surface; native components themselves cannot simply be shared as DOM widgets.

## Component contracts and UI state

Each independently reusable component gets a module and a short contract: purpose, NSH dependency, typed inputs/intents, supported variants/states, controlled versus local state, layout/overflow behavior, focus/keyboard/accessibility behavior, and gallery/regression cases. Define only useful variants, rather than a universal component with arbitrary flags or unrestricted styles that bypass its contract.

For example, a Button owns its appearance and interaction contract; a CardTile owns title/metadata/status presentation and emits open/move intents. The screen/presenter performs an AppServer operation. Neither the tile nor generic Button launches an Agent or writes SQLite. Parent screens control placement; shared components control their own semantic styling and behavior.

Keep persistent state separate from UI state. AppServer supplies accepted card/run state; UI owns navigation, focus, selection and drafts. A presenter translates async operations into pending/success/failure feedback, unregisters subscriptions on disposal, and prevents a late response for one Card from replacing the view for another. Business eligibility is still validated by AppServer. Components expose these states so the gallery can reproduce them without a live Agent.

## Client-transport and terminal decoupling boundaries

Architecture direction, 2026-11-21: keep `canban-ui` and `canban-terminal` decoupled from local daemon processes, local SQLite storage, and platform-specific view toolkits so desktop and future remote-only/mobile clients (such as iOS) share the same core UI and terminal stack.

```text
canban-ui (pure UI core: --no-default-features)
  Screens / Presenters / Components / UiBackend trait
       |                                    |
       v                                    v
  [feature: local-host]              canban-terminal (core: --no-default-features)
    desktop startup / updater          model.rs (libghostty-vt) + view_state.rs
    canban-app-server / HostRegistry   LiveSession LRU pool + PtyTransport trait
    local socket & child processes          |                        |
                                            v                        v
                                    [feature: herdr-local]    Renderer / View host
                                      canban-herdr / hc         apple_render.m (CG/CT)
                                                                + macos_view.m (AppKit)
                                                                / linux.rs (NSH canvas)
```

- **`UiBackend` boundary in `canban-ui`:** All RPC routing (`routed_call`, `call_all_hosts`), merged snapshots, host summaries, host configs, event subscriptions, and draft persistence route through a single `UiBackend` trait instead of scattered `APP` / `HOSTS` globals or direct `std::fs` calls. Desktop-only process startup (`startup.rs`), local self-updates (`updates.rs`), `signal-hook`, `canban-exec`, `canban-updater`, and `canban-app-server` sit behind the `local-host` Cargo feature (enabled by default for desktop builds). `cargo check -p canban-ui --no-default-features` must remain free of local server, SQLite, and subprocess crates.
- **`PtyTransport` boundary in `canban-terminal`:** `LiveSession`, the bounded LRU session pool (`MAX_POOLED_SESSIONS`), and `model.rs` communicate with PTYs through a transport-agnostic `PtyTransport` trait (`attach`, `write`, `resize`, `detach`). The local `hc` implementation (`HerdrLocalTransport`) sits behind the optional `herdr-local` Cargo feature so remote WebSocket/stream transports can plug into the same session pool and VT engine without pulling `canban-herdr` or `canban-exec`.
- **Layered terminal view and renderer:** Platform-independent terminal view state (dirty-row snapshot polling, cursor blink/animation timing, optimistic CJK IME commit cursor hold, and link hover state) belongs in pure Rust (`crates/terminal/src/view_state.rs`). On Apple platforms, CoreGraphics/CoreText rendering (`apple_render.m`: vector box/block/Powerline glyphs, font fallback, underlines, Kitty image layers, and grid draw loop using `CGRect`/`CGColorRef` with zero AppKit imports) stays separate from the thin platform view host (`macos_view.m` for AppKit `NSView`/`NSTextInputClient`/drag-and-drop; future `ios_view.m` for UIKit).

## State synchronization, ownership and memory cost

The UI contract includes the cost of supplying data to components, not only the
cost of drawing them. AppServer owns database projections and pagination;
AppClient/HostRegistry own transport, host caches and invalidation; presenters
own coalesced refresh and resource lifetimes; UI indexes own derived lookups.
UI Kit remains independent of application state and persistence.

### Current synchronization and ownership

Implemented on 2026-11-17; [memory profiling and synchronization](memory-profiling.md)
contains source-level details, diagnostic commands and dated acceptance evidence.

```text
Local AppClient -- app.watch(cursor) --> AppServer: small cursor checks
                 <-- changed cursor --
Presenter ------ app.state ----------> current full-state projection
                 <-- owned response --
                         |
                    UI Snapshot
                         |
              event positions / summaries
                         |
              selected Card history view

Remote supervisor -- app.state(after: cursor) --> remote AppServer
                  <-- unchanged metadata OR updated full state --
                         |
                   typed offline cache
```

Local change detection does not fetch and retain a full JSON state merely to
compare it and trigger another full fetch. Remote supervision retains cursor
metadata beside its typed cache. Owned JSON response fields are moved into the
next decoding stage; the global event index stores positions instead of another
copy of every event body. Timeline RPC pages select and limit rows in SQL before
JSON decoding, and background runtime decisions use narrower projections.

A changed state can still require a full Snapshot, and concurrent updates can
require another refresh. Runtime freshness can invalidate the cursor. This is
not per-entity delta synchronization: full state still includes history used by
reminders and other consumers, selected-Card history is materialized, and merged
remote typed snapshots still involve copies. Compact global summaries and fully
lazy history loading remain follow-up work.

### Review contract for new or changed data paths

- **Invalidate cheaply and refresh through the owner.** Share the existing
  per-host synchronization path, coalesce invalidations and allow one presenter
  refresh at a time. Rows and components must not add independent full-state
  polling or repeated per-row RPCs. Distinguish transport liveness, observation
  freshness and changed application content. Channels and cursors invalidate UI
  state; they do not replace durable business facts or authorize actions.
- **Make consistency and recovery explicit.** Pair a cursor with the state it
  describes, and never let an older response acknowledge a newer commit. Keep
  persistent server identity distinct from a fresh boot instance. Cover internal
  worker writes, configuration and transient state changes, reconnects and stale
  responses. Positional indexes must be consumed with their source Snapshot;
  generation/resource fences protect async work and retained views.
- **State who owns large values.** Borrow or move owned payloads before adding a
  deep clone. Derived indexes should store IDs, positions, compact summaries or
  suitable shared references rather than duplicate complete histories. `Rc`/`Arc`
  share data only within a process; cross-process serialization and typed decoding
  still have costs. Document intentional copies and their lifetime.
- **Bound reads at the storage boundary.** Select the required records and fields
  for the consumer; filter, order and limit history in SQL before decoding.
  Returning a page after loading the entire table is not bounded loading.
  Background queue/deadline checks must not pull unrelated UI/history payloads.
- **Bound retained data as well as query work.** Give caches an owner, disposal
  lifetime, invalidation/reconnect policy and capacity. For variable-size bodies,
  consider byte budgets in addition to entry counts; a count limit alone does not
  establish a memory bound. Reuse mounted views and valid derived results when
  unrelated metadata changes, while releasing disposed subscriptions and work.
- **Preserve observable semantics while narrowing data.** Keep unread positions,
  reply/report deduplication, pending reminders, runtime uncertainty, offline-host
  behavior, draft/scroll/focus retention and complete history access correct.
  Replace history-dependent consumers with equivalent summaries before removing
  their inputs. Silently truncating history or clearing caches without an
  ownership/invalidation policy is not an acceptable memory fix.

### Cost verification

For changes to synchronization, indexes, history loading or caches, verify both
unchanged and changed data, internal writes, reconnect/restart and the affected
native view. Repeat the same Card navigation, Terminal switching and output
workload when investigating growth. Report allocation volume, live bytes,
physical footprint/peak, fragmentation and CPU separately; a lower response byte
count or one RSS sample does not establish that a leak was fixed or that the
workload reaches a stable memory plateau.

Use opt-in Instruments intervals to attribute database reads, RPC encoding and
decoding, UI apply, indexing and Terminal attachment to the owning layer. A trace
file is evidence only when attachment/recording actually succeeded; retain the
binary identity, capture status and workload. Gallery interaction checks and
native integration checks keep their existing roles. Documentation-only updates
add no runtime acceptance and do not require repeating an unrelated test suite.

## Interaction responsiveness, scroll anchoring and NSH notes

- **Non-blocking screens and mutations:** Heavy reads (such as Settings probes) and slow operations (such as Review deliveries or new Terminal launches) run asynchronously with explicit loading/pending feedback rather than blocking the UI thread or waiting on full workspace refreshes. Interactive mutations apply optimistic projections and hydrate the complete write receipt on completion; gate controls on method-scoped `ui.is_method_pending(...)` rather than global `ui.pending`, and route card/update mutations through `submission::immediate()` or `config_only()`.
- **Narrow reactive granularity and batched transitions:** Never read high-frequency signals (`ui.runtime_clock`, raw text-input signals, or full `ui.snapshot`) inside an outer `dynamic` that builds a whole screen, header, or list row — push clocks and input validation into leaf closures (`label(move || ...)`, `.disabled(move || ...)`) and omit unused fields from `PartialEq` view models (such as `CardTileModel`). Wrap multi-signal navigation, tab-switch, and pane-toggle updates in `batch(|| { ... })` to avoid intermediate reactive flushes.
- **Cache-first and flicker-free loading:** Hydrate initial workspace mount from the bootstrap snapshot instead of mounting with `Snapshot::default()` and refetching. Check warm in-memory caches synchronously (`peek_fresh`) before clearing existing state or entering `Loading`, and during debounced filter/tab changes keep stale rows visible or enter `loading = true` immediately rather than clearing to an empty state before the debounce fires.
- **In-page native PTY retention:** On `Terminals`, keep the single `native("canban/terminal")` host mounted while on-page (hiding it with `binding = Null` and an overlay placeholder during empty/connecting states, and keeping `binding_key` separate from `read_only`/`input_status`) to avoid cold re-init flicker; continue unmounting the native view off-page so memory remains bounded and single-active-PTY ownership with the Card split is preserved.
- **Event-driven or backed-off background timers:** Avoid unconditional high-frequency `set_interval` polling on the UI thread. Use atomic change epochs or push invalidations when idle (for example, draft save errors), skip polling for disabled features, and back off utility/update polling when popovers are closed or idle.
- **Scroll anchoring, restoration and dynamic expansion:** Surfaces whose content height changes above or inside the viewport — including foldable Terminals trees, progressive Diff context expansion, and expandable Card comment reports — must preserve scroll anchors (including in short scroll views) or pause tail-follow while expanding. Restore saved scroll positions synchronously via `.content_offset(pos)` on `scroll()` rather than deferring `scroll_to` to `next_frame().await` in `on_layout` (which flashes `y = 0` for one frame).
- **Drag and motion consistency:** Keep dragged Board cards mounted/visible until the drag ghost mounts, and route both drag drops and non-drag Card moves/exits through one unified flight-motion path. All clickable targets use pointer cursors consistently. See [Native UI motion](ui-motion.md) for shared overlay, split-reveal, and control-transition patterns.
- **Version-scoped NSH workarounds:** Track NSH-specific runtime quirks (reactive theme tracking, macOS CoreText system-monospace CJK spacing, IME buffers, scroll culling, hover cursor stability, and custom scrollbars) in [NSH integration notes](nsh-notes.md) pinned to the current NSH version/revision so upgrades can audit and retire obsolete workarounds.

## Native component gallery

Create a Rust/NSH gallery using the exact components imported by production screens. NSH's [Rust gallery](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/examples/gallery-rs/README.md) demonstrates a Rust-only native entry. Canban's Gallery is a standalone Cargo example at `examples/gallery`, importing production components from the `canban-ui` library with local fixtures. Run it with `mise run gallery`; it opens `.local/CanbanGallery.app` without an AppServer, database or Agent. The production desktop has no Gallery route or startup mode. This allows both UI Kit and product scenarios without making UI Kit depend on product types. It is not a new production service or a parallel implementation of the controls.

Show variants and meaningful states, including named-theme preview/confirmation, keyboard focus, disabled/pending/error, long and Chinese text, narrow/large windows, empty and populated lists, long reports, and archive-loading/failure. Add combined scenes for menu inside Card detail, composer inside a scrollable Timeline, and Board movement. Isolated buttons alone will not reveal focus, clipping and scrolling defects caused by composition.

Gallery fixtures are deterministic and do not start real Agents. UI component changes are checked through this standalone example; add a scene for a new component or regression. Business-operation changes use the relevant integration acceptance separately. Gallery checks establish the exercised component behavior, not AppServer integration. See [example development instructions](../examples/README.md).

## Development and regression loop

1. Describe the product interaction and its states; look up the component inventory and NSH reuse route.
2. Reuse an existing component, add a focused variant, or introduce a component with a documented reason. Do not copy its implementation into a screen.
3. Develop the shared component and a realistic gallery scene, then compose the screen using it.
4. Verify the actual affected behavior. Use targeted Rust tests for meaningful state/intent logic; deterministic interaction tests where the harness supports them; visual comparisons for layout/theme; and native acceptance for focus, IME, drag/scroll and platform behavior.
5. When a defect appears, locate its owning layer. Fix generic NSH behavior in an NSH patch/upstream route or one isolated adapter; fix shared Canban behavior in UI Kit/product components; fix screen composition in the screen. Add the original failing scenario where useful, and check other known consumers of the changed component.

Do not claim visual/interaction acceptance from compilation or screenshots alone. Verify the running build/process before taking native evidence; historical desktop work exposed stale-process screenshot mistakes. Use changed-scope checks rather than testing every static leaf or running a huge matrix for each cosmetic edit. Shared control, NSH version and global-token changes justify checking representative consumers and compositions.

The component inventory should name its owner module, reuse route, consumers, and gallery/verification coverage, with gaps visible. A development review asks whether shared controls were reused, whether a new variant is justified, and whether the changed behavior was verified. These are selected review criteria; no automatic enforcement system exists yet. Lightweight dependency/style checks can help once the conventions are concrete; do not rely on a large grep blacklist to establish UI correctness.

## Build order

Start with a small shell and core token/typography/control set. Build a native vertical slice: Board CardTile -> Card detail -> comment/mention composer -> Timeline feedback, including keyboard and async states. Use this to prove overlay, input, scrolling and state-update composition. Extend the same system for Overview, Terminals and Review rather than introducing parallel control implementations. Notes are [deferred](notes-deferred.md) pending a separate product design.

Do not build an exhaustive speculative widget library before the first useful flow. Extract reusable units with explicit contracts as real needs appear; one-off layout can stay local. The desired outcome is one maintained implementation of each shared behavior, with defects fixed once and retained as regressions. No architecture can guarantee zero UI defects; this structure limits duplicated causes and makes fixes propagate.

## Selected direction and implementation refinements

The user approved separating UI Kit from product UI, defaulting to suitable NSH behavior reuse, maintaining a native shared-component gallery, and treating component-level fixes plus meaningful regression scenarios as part of feature completion. The crate names follow canban-ui-kit/crates/ui-kit and canban-ui/crates/ui. The named-theme contract and current component inventory now record the implemented choices. Further density, component and tooling refinements follow concrete product needs. Architecture guidance alone does not establish runtime acceptance or expand product scope.
