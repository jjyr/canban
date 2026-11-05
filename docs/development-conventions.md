# Development conventions

Status: user-selected conventions, 2026-11-06. The repository currently contains design documents; this guide applies when implementation manifests and source files are created.

## Cargo package names and filesystem names

Use canban-xxx for Cargo package names. Use the unprefixed xxx for crate directories and ordinary source/module filenames.

```text
crates/
  app-server/        package: canban-app-server
  kanban/            package: canban-kanban
  workflow-engine/   package: canban-workflow-engine
  scheduler/         package: canban-scheduler
  context/           package: canban-context
  timeline/          package: canban-timeline
  notifications/    package: canban-notifications
  herdr/             package: canban-herdr
  workspaces/        package: canban-workspaces
  storage/           package: canban-storage
  types/             package: canban-types
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

These are configuration examples, not created manifests. Binary command names remain an interface/packaging choice and do not have to match the Cargo package name.

## License

Canban is licensed under Apache-2.0. The root [LICENSE](../LICENSE) contains the standard Apache License, Version 2.0 text. Use the SPDX identifier Apache-2.0 in Cargo metadata; future workspace packages inherit it with license.workspace = true.

Original Canban copyright attribution: Copyright 2026 JJy <jjyruby@gmail.com>. The project notice appears above the complete, unchanged Apache-2.0 license text; the standard appendix remains part of that text.

Retain existing licenses, copyright notices, and applicable third-party notices when importing external code/assets. Selecting Canban's project license does not replace the original notices of imported code.

## Architecture references

The [crate plan](architecture-crates-and-timers.md) defines proposed responsibility/dependency boundaries, and the [overall architecture](architecture.md) defines the co-hosted AppServer/native UI/runtime relationship. Naming conventions are selected; the workspace and application have not yet been implemented.
