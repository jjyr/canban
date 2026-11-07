# Development conventions

Status: user-selected conventions, 2026-11-06, applied by the initial Rust workspace and macOS MVP.

## Cargo package names and filesystem names

Use canban-xxx for Cargo package names. Use the unprefixed xxx for crate directories and ordinary source/module filenames.

```text
crates/
  app-server/        package: canban-app-server
  kanban/            package: canban-kanban
  workflow-engine/   package: canban-workflow-engine
  scheduler/         package: canban-scheduler
  profiles/          package: canban-profiles
  terminal/          package: canban-terminal
  context/           package: canban-context
  timeline/          package: canban-timeline
  notifications/    package: canban-notifications
  herdr/             package: canban-herdr
  workspaces/        package: canban-workspaces
  storage/           package: canban-storage
  types/             package: canban-types
  ui-kit/            package: canban-ui-kit
  ui/                package: canban-ui
  cli/               package: canban-cli
```

For example: crates/scheduler/Cargo.toml declares name = "canban-scheduler". Its source files can be src/lib.rs, src/queue.rs, and src/timers.rs; do not add canban- to those filesystem names. Normal Rust imports use the package-derived underscore form, such as canban_scheduler.

Workspace dependencies use package names as keys and unprefixed paths. Illustrative fragments for the future workspace:

```toml
# Root Cargo.toml
[workspace.package]
license = "Apache-2.0"

[workspace.dependencies]
canban-scheduler = { path = "crates/scheduler" }
```

```toml
# crates/scheduler/Cargo.toml
[package]
name = "canban-scheduler"
license.workspace = true
```

These fragments illustrate conventions now used by the workspace manifests. The Agent CLI binary is canban; the desktop binary is canban-ui.

## UI architecture and component development

Follow the user-approved [UI architecture and design system](ui-architecture.md).

- Keep generic tokens/styled controls and shared interaction conventions in canban-ui-kit. Keep product components, presenters and screen composition in canban-ui. UI Kit has no AppServer, persistence, Herdr or product-domain dependency.
- Reuse suitable NSH controls/behavior before implementing custom interaction. Give independently reusable components their own file/module and a concise typed input/event, state, layout and interaction contract; do not make a crate per widget or copy shared controls into screens.
- Maintain the standalone native Rust/NSH Gallery in `examples/gallery`, importing production components with deterministic fixtures and meaningful composition states. Use `mise run gallery` for UI component checks; keep its entry/fixture data out of the production app.
- Reuse shared tokens and explicit component variants. Extend the shared design contract when a new semantic need arises rather than applying inconsistent screen-specific overrides.
- Fix defects in their owning layer, retain useful reproductions/regression cases and verify affected consumers. Compilation or screenshots alone do not establish native focus, IME, drag or scroll acceptance. Use verification appropriate to the change.
- Build the system through a useful Board -> Card detail -> comment/mention -> Timeline slice, then extend it with subsequent product surfaces. Do not build an exhaustive speculative component library first.

The implementation now includes UI Kit, a native Gallery and an opt-in real-Herdr acceptance harness. See [component inventory](ui-components.md) and [implementation/acceptance](implementation.md) for coverage; further components and tooling remain refinements.

## License

Canban is licensed under Apache-2.0. The root [LICENSE](../LICENSE) contains the standard Apache License, Version 2.0 text. Use the SPDX identifier Apache-2.0 in Cargo metadata; future workspace packages inherit it with license.workspace = true.

Original Canban copyright attribution: Copyright 2026 JJy <jjyruby@gmail.com>. The project notice appears above the complete, unchanged Apache-2.0 license text; the standard appendix remains part of that text.

Retain existing licenses, copyright notices, and applicable third-party notices when importing external code/assets. Selecting Canban's project license does not replace the original notices of imported code.

## Architecture references

The [crate plan](architecture-crates-and-timers.md) defines proposed responsibility/dependency boundaries, and the [overall architecture](architecture.md) defines the co-hosted AppServer/native UI/runtime relationship. The workspace now implements the core boundaries; timer placement and other deferred capabilities remain design proposals.
