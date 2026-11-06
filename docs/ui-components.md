# UI component inventory

Status: initial implementation inventory, 2026-11-06. These modules are used by the native application; the Gallery imports the same implementations.

| Component/layer | Module | NSH reuse and contract | Verification |
| --- | --- | --- | --- |
| Semantic tokens | crates/ui-kit/src/tokens.rs | Shared adaptive surfaces/text/accent/borders and sizing | Native light/dark Gallery and Board inspected |
| Button | crates/ui-kit/src/button.rs | nsh-base Button; typed variants, centralized hover/pressed/focus/disabled styles | Native mouse, Tab/Space and disabled-skip checks |
| Text field | crates/ui-kit/src/input.rs | Controlled NSH Input; common geometry/focus/text | Multiline Unicode paste/edit/save, live Markdown preview and retained draft through state refresh |
| Checkbox | crates/ui-kit/src/checkbox.rs | Controlled nsh-base Checkbox; common checked/focus presentation | Used by per-Card Note selection; snapshot semantics have Rust regression coverage |
| Panels/text/badge | crates/ui-kit/src/surface.rs | Shared NSH layout/text composition | Native themes and real Card composition inspected |
| Card tile | crates/ui/src/components.rs | Product projection, press-to-open, dedicated pan handle and cancellation | Native Backlog -> Develop drag invoked real scheduling |
| Timeline item | crates/ui/src/components.rs | Comment Markdown, event provenance, report/output intents | Production and Gallery fixtures; explicit reports through real Herdr/CLI |
| Agent run/output panel | crates/ui/src/components.rs | Request/attempt view, retry and read-only output | Fixture stages and real Codex completion; full terminal takeover deferred |
| Context picker | crates/ui/src/components.rs | Per-Card Project Note selection via shared Checkbox | Immutable selected-context regression; native checkbox toggle and saved revision verified |
| Presenters | crates/ui/src/presenter.rs | Async AppServer operations, event-driven refresh, draft preservation and resource generation checks | Live state updates, native saves and image-paste refresh |
| Screens / Gallery | crates/ui/src/screens.rs | Compose production components; fixture-only Gallery requires no model | Board, Card, Notes and Gallery native checks |

Components use their current typed Rust inputs/events; longer-form component API docs can grow with the public surface. Menus/dialogs/rich-editor/virtual-list wrappers should be added when a concrete feature needs them, using NSH facilities first. Do not create parallel screen copies or claim unimplemented Gallery scenarios as tested. The [design-system conventions](ui-architecture.md) govern subsequent changes.
