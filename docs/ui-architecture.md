# UI architecture and design system

Status: user-approved architecture and development direction, 2026-11-06, now applied in the first native MVP. The approved UI Kit/product UI separation, suitable NSH reuse, component contracts, native gallery and component-level regression workflow guide implementation. Further components and visual refinements can extend the initial system.

Current contract, reconciled 2026-11-10: the native UI, shared UI Kit and production-component Gallery exist. This document governs component ownership and interaction design; [implementation](implementation.md), the [component inventory](ui-components.md) and feature documents identify actual behavior and acceptance. Feasibility/research examples below do not establish runtime coverage.

## Architecture

```text
canban-ui
  Screens: Board / Card conversation / Overview / Terminals / Review
       |
  Product components: CardTile / StageColumn / TimelineItem
                      AgentRunPanel / MentionComposer / ReportViewer
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
Board (Project work and workflow)
  +-- Card: task/context + one scrolling conversation/Timeline
  +-- Review: report, Diff and explicit delivery/rework

Overview (all Projects)
  +-- grouped Agents, human decisions and exact destination actions

Terminals (direct interaction)
  +-- managed Card/profile Agents + independent manual Shells

```

Board is the management center. Overview gathers cross-Project execution and human actions; Terminals owns direct PTY interaction and Agent lifecycle controls. Card conversation keeps task/context and history in one scroll region, with compact fixed controls and a reachable composer. Project creation/selection belongs in Board; the rail stays compact. See [Card composition](card-conversation.md) and [Terminal workspace](terminal-workspace.md) for the exact layouts.

- Put task identity and current actionable state first. Omit repeated drag instructions, empty execution labels, duplicate stage labels and permanent technical captions. Keep failure and human-action feedback discoverable; quiet presentation must not conceal a failed Agent.
- Display frequently needed context inline and disclose secondary details on demand. Terminal list rows show title/stage, Provider/Profile, state and machine without metadata popovers or current-directory text. Branch full-value tooltips and secondary menus remain useful. This is not a blanket ban on hover: Theme intentionally previews on hover, then saves on confirmation.
- Reserve essential controls before allocating flexible title space. Test long names, mixed-language titles, narrow windows, overflow and empty/error/pending states. Truncate compact rows; offer full text through selection, expansion or an appropriate tooltip according to the component contract.
- Use one operation path for button, shortcut and menu. New Card Create and comment Send share Command+Enter as a convention, with their own scoped validity/pending checks. Escape/outside dismissal and focus return follow the owning overlay's contract.
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

Define applicable states for each control: default, hover, pressed, keyboard focus, disabled, pending and error. Define composition behavior: focus entry/return, Escape and outside-click handling, shortcut scope, text selection, copy/paste, IME composition, overflow/truncation, minimum sizing and scroll ownership. Not every component needs every state, but omitted behavior should be intentional.

Cross-cutting focus/overlay/command conventions belong in shared UI facilities on top of NSH, not independently in each screen. This need not become a new framework; reuse NSH's mechanisms and centralize only the app policy/adaptation required. For optional WebViews, map the same semantic tokens and explicit focus/keyboard/bridge contracts into the embedded surface; native components themselves cannot simply be shared as DOM widgets.

## Component contracts and UI state

Each independently reusable component gets a module and a short contract: purpose, NSH dependency, typed inputs/intents, supported variants/states, controlled versus local state, layout/overflow behavior, focus/keyboard/accessibility behavior, and gallery/regression cases. Define only useful variants, rather than a universal component with arbitrary flags or unrestricted styles that bypass its contract.

For example, a Button owns its appearance and interaction contract; a CardTile owns title/metadata/status presentation and emits open/move intents. The screen/presenter performs an AppServer operation. Neither the tile nor generic Button launches an Agent or writes SQLite. Parent screens control placement; shared components control their own semantic styling and behavior.

Keep persistent state separate from UI state. AppServer supplies accepted card/run state; UI owns navigation, focus, selection and drafts. A presenter translates async operations into pending/success/failure feedback, unregisters subscriptions on disposal, and prevents a late response for one Card from replacing the view for another. Business eligibility is still validated by AppServer. Components expose these states so the gallery can reproduce them without a live Agent.

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
